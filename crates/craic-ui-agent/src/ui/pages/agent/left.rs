use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::ui::agent_history::{self, AgentSessionRow, RestoreState, WorkspaceKey, WorkspaceTag};
use crate::ui::agent_status::{AgentActiveState, AgentInactiveState, AgentSessionState};
use crate::ui::agent_usage::AgentResourceUsage;
use crate::ui::canvas_scroll;
use crate::ui::components::context_menu::{self, ActionMenuItem, ActionMenuSection};
use crate::ui::components::search::{SearchPanel, SearchTag};

use super::{
    AGENT_ICON_PIXEL_SIZE,
    provider::{self, AgentProvider},
};

const HISTORY_PAGE_SIZE: usize = 32;
const HISTORY_PREFETCH_DISTANCE: f64 = 360.0;
const HISTORY_DB_REFRESH_DEBOUNCE: Duration = Duration::from_millis(150);
const HISTORY_DB_MONITOR_RATE_LIMIT_MS: i32 = 250;
const HISTORY_AGENT_SESSION_ICON_OPACITY: f64 = 0.58;
const ACTIVE_MISSING_CLI_SESSION_ID_LABEL: &str = "No session ID yet";
const ACTIVE_MISSING_CLI_SESSION_ID_TOOLTIP: &str =
    "This active chat has not been mapped to a CLI session ID yet.";
const WAITING_AGENT_SESSION_ICON: &str = "hand-touch-symbolic";
const UNRESTORABLE_AGENT_SESSION_ICON: &str = "background-app-ghost-symbolic";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentListSelection {
    Active(u64),
    History(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentLaunch {
    App,
    CodexCli,
    Agy,
}

impl AgentLaunch {
    pub fn terminal_provider(self) -> Option<&'static dyn AgentProvider> {
        match self {
            AgentLaunch::App => None,
            AgentLaunch::CodexCli => Some(&provider::codex::PROVIDER),
            AgentLaunch::Agy => Some(&provider::agy::PROVIDER),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentListContextAction {
    ViewStatusHistory(i64),
    ViewStatusActive(u64),
    GenerateSummaryHistory(i64),
    GenerateSummaryActive(u64),
    SetSessionIdHistory(i64),
    SetSessionIdActive(u64),
    Unload(i64),
    Delete(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentListContextTarget {
    session_id: Option<u64>,
    local_id: Option<i64>,
    loaded: bool,
    has_summary: bool,
    terminal_session: bool,
}

#[derive(Clone)]
pub struct AgentList {
    pub root: gtk::Box,
    app_button: gtk::Button,
    codex_cli_button: gtk::Button,
    agy_button: gtk::Button,
    search_panel: SearchPanel,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    suppress_selection_callback: Rc<Cell<bool>>,
    selection_callback: Rc<RefCell<Option<Rc<dyn Fn(AgentListSelection)>>>>,
    context_action_callback: Rc<RefCell<Option<Rc<dyn Fn(AgentListContextAction)>>>>,
    close_callback: Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    active_sessions: Rc<RefCell<Vec<ActiveSessionInfo>>>,
    workspace: Rc<RefCell<Option<WorkspaceKey>>>,
    history_rows: Rc<RefCell<Vec<AgentSessionRow>>>,
    search_query: Rc<RefCell<String>>,
    selected_tags: Rc<RefCell<HashSet<String>>>,
    row_widgets: Rc<RefCell<HashMap<AgentRowKey, gtk::ListBoxRow>>>,
    row_states: Rc<RefCell<HashMap<AgentRowKey, AgentRowRenderState>>>,
    loaded_limit: Rc<Cell<usize>>,
    has_more: Rc<Cell<bool>>,
    loading: Rc<Cell<bool>>,
    history_monitor: Rc<RefCell<Option<gio::FileMonitor>>>,
    debounce_source: Rc<RefCell<Option<glib::SourceId>>>,
    history_db_signature: Rc<RefCell<HistoryDbSignature>>,
    history_monitor_stats: Rc<RefCell<HistoryMonitorStats>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HistoryDbSignature {
    database: Option<HistoryDbFileSignature>,
    wal: Option<HistoryDbFileSignature>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryDbFileSignature {
    len: u64,
    modified: Option<SystemTime>,
}

struct HistoryMonitorStats {
    started_at: Instant,
    raw_events: u64,
    suppressed_events: u64,
    reloads: u64,
}

impl Default for HistoryMonitorStats {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            raw_events: 0,
            suppressed_events: 0,
            reloads: 0,
        }
    }
}

#[derive(Clone)]
struct ActiveSessionInfo {
    session_id: u64,
    provider: &'static dyn AgentProvider,
    title: String,
    local_history_id: Option<i64>,
    state: AgentSessionState,
    usage: Option<AgentResourceUsage>,
    last_seen_at_ms: i64,
}

struct AgentHistoryLoad {
    workspace_key: String,
    search_query: String,
    selected_tags: Vec<String>,
    loaded_limit: usize,
    rows: Result<Vec<AgentSessionRow>, String>,
    tags: Option<Result<Vec<WorkspaceTag>, String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowIdentity {
    Active(u64),
    History(i64),
    Header,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum AgentRowKey {
    Active(u64),
    History(i64),
    Header(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AgentRowRenderState {
    Active {
        session_id: u64,
        provider_label: &'static str,
        provider_icon_name: &'static str,
        title: String,
        state: AgentSessionState,
        usage_label: Option<String>,
        missing_cli_session_id: bool,
    },
    History {
        local_id: i64,
        provider_label: &'static str,
        provider_icon_name: &'static str,
        title: String,
        time_label: String,
        inactive_state: AgentInactiveState,
        restore_state: RestoreState,
    },
    Header {
        label: String,
    },
}

include!("left/list.rs");

fn show_agent_session_context_menu(
    agent_list: &AgentList,
    target: AgentListContextTarget,
    x: f64,
    y: f64,
) {
    let popover = context_menu::popup_action_menu(
        &agent_list.list,
        x,
        y,
        agent_session_context_menu_sections(target),
        {
            let context_action_callback = agent_list.context_action_callback.clone();
            move |action| {
                if let Some(ref cb) = *context_action_callback.borrow() {
                    cb(action);
                }
            }
        },
    );
    retain_context_menu(
        &agent_list.active_context_menu,
        popover.upcast_ref::<gtk::Popover>(),
    );
}

fn agent_session_context_menu_sections(
    target: AgentListContextTarget,
) -> Vec<ActionMenuSection<AgentListContextAction>> {
    let view_action = match (target.local_id, target.session_id) {
        (Some(local_id), _) => AgentListContextAction::ViewStatusHistory(local_id),
        (None, Some(session_id)) => AgentListContextAction::ViewStatusActive(session_id),
        (None, None) => return Vec::new(),
    };
    let set_session_id_action = match (target.local_id, target.session_id) {
        (Some(local_id), _) => AgentListContextAction::SetSessionIdHistory(local_id),
        (None, Some(session_id)) => AgentListContextAction::SetSessionIdActive(session_id),
        (None, None) => return Vec::new(),
    };
    let summary_action = match (target.local_id, target.session_id) {
        (Some(local_id), _) => AgentListContextAction::GenerateSummaryHistory(local_id),
        (None, Some(session_id)) => AgentListContextAction::GenerateSummaryActive(session_id),
        (None, None) => return Vec::new(),
    };
    let summary_label = if target.has_summary {
        "Regenerate Summary"
    } else {
        "Generate Summary"
    };

    let mut session_items = vec![ActionMenuItem::new("View Status", view_action, true)];
    if target.terminal_session {
        session_items.push(ActionMenuItem::new(
            summary_label,
            summary_action,
            target.loaded,
        ));
        session_items.push(ActionMenuItem::new(
            "Set Session ID...",
            set_session_id_action,
            true,
        ));
    }
    if let Some(local_id) = target.local_id {
        session_items.push(ActionMenuItem::new(
            "Unload Session",
            AgentListContextAction::Unload(local_id),
            target.loaded,
        ));
    }

    let mut sections = vec![ActionMenuSection::new(session_items)];
    if let Some(local_id) = target.local_id {
        sections.push(ActionMenuSection::new(vec![ActionMenuItem::new(
            "Delete Session...",
            AgentListContextAction::Delete(local_id),
            true,
        )]));
    }
    sections
}

fn retain_context_menu(
    active_context_menu: &Rc<RefCell<Option<gtk::Popover>>>,
    popover: &gtk::Popover,
) {
    if let Some(existing) = active_context_menu.borrow_mut().replace(popover.clone()) {
        existing.popdown();
        existing.unparent();
    }
}

enum TimelineRow<'a> {
    Active(&'a ActiveSessionInfo),
    History(&'a AgentSessionRow),
}

impl TimelineRow<'_> {
    fn last_seen_at_ms(&self) -> i64 {
        match self {
            TimelineRow::Active(session) => session.last_seen_at_ms,
            TimelineRow::History(row) => row.last_seen_at_ms,
        }
    }

    fn identity_order(&self) -> u64 {
        match self {
            TimelineRow::Active(session) => session.session_id,
            TimelineRow::History(row) => u64::try_from(row.id).unwrap_or(0),
        }
    }
}

fn new_agent_button(label_text: &str) -> gtk::Button {
    let icon = gtk::Image::from_icon_name("list-add-symbolic");
    icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);

    let label = gtk::Label::new(Some(label_text));

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .build();
    content.append(&icon);
    content.append(&label);

    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text(format!("New {label_text} chat"))
        .halign(gtk::Align::Center)
        .build();
    button.add_css_class("flat");
    button
}

fn sorted_tags(tags: &HashSet<String>) -> Vec<String> {
    let mut tags = tags.iter().cloned().collect::<Vec<_>>();
    tags.sort();
    tags
}

fn active_row_render_state(
    session: &ActiveSessionInfo,
    history_rows: &[AgentSessionRow],
) -> AgentRowRenderState {
    AgentRowRenderState::Active {
        session_id: session.session_id,
        provider_label: session.provider.label(),
        provider_icon_name: session.provider.session_icon_name(),
        title: if session.state == AgentSessionState::Active(AgentActiveState::NewChat) {
            session.provider.default_title()
        } else {
            session.title.clone()
        },
        state: session.state,
        usage_label: session.usage.map(|usage| usage.sidebar_label()),
        missing_cli_session_id: active_session_missing_cli_session_id(session, history_rows),
    }
}

fn history_row_render_state(session: &AgentSessionRow) -> AgentRowRenderState {
    AgentRowRenderState::History {
        local_id: session.id,
        provider_label: provider_label(&session.provider_id),
        provider_icon_name: provider_icon_name(&session.provider_id),
        title: session.title.clone(),
        time_label: history_time_label(session.last_seen_at_ms),
        inactive_state: history_inactive_state(session.restore_state),
        restore_state: session.restore_state,
    }
}

fn agent_row(state: &AgentRowRenderState, close_callback: Rc<dyn Fn(u64)>) -> gtk::ListBoxRow {
    match state {
        AgentRowRenderState::Active {
            session_id,
            provider_label,
            provider_icon_name,
            title,
            state,
            usage_label,
            missing_cli_session_id,
        } => active_chat_row_for_state(
            *session_id,
            provider_label,
            provider_icon_name,
            title,
            *state,
            usage_label.as_deref(),
            *missing_cli_session_id,
            close_callback,
        ),
        AgentRowRenderState::History {
            local_id,
            provider_label,
            provider_icon_name,
            title,
            time_label,
            inactive_state,
            restore_state,
        } => history_chat_row_for_state(
            *local_id,
            provider_label,
            provider_icon_name,
            title,
            time_label,
            *inactive_state,
            *restore_state,
        ),
        AgentRowRenderState::Header { label } => section_header_row(label),
    }
}

fn update_agent_row(
    widget: &gtk::Widget,
    state: &AgentRowRenderState,
    close_callback: Rc<dyn Fn(u64)>,
) {
    let Ok(row) = widget.clone().downcast::<gtk::ListBoxRow>() else {
        return;
    };
    if row_render_kind(&row) != render_state_kind(state) {
        replace_row_child(&row, agent_row(state, close_callback));
    }
    match state {
        AgentRowRenderState::Active {
            session_id,
            provider_label,
            provider_icon_name,
            title,
            state,
            usage_label,
            missing_cli_session_id,
        } => update_active_row(
            &row,
            *session_id,
            provider_label,
            provider_icon_name,
            title,
            *state,
            usage_label.as_deref(),
            *missing_cli_session_id,
        ),
        AgentRowRenderState::History {
            local_id,
            provider_label,
            provider_icon_name,
            title,
            time_label,
            inactive_state,
            restore_state,
        } => update_history_row(
            &row,
            *local_id,
            provider_label,
            provider_icon_name,
            title,
            time_label,
            *inactive_state,
            *restore_state,
        ),
        AgentRowRenderState::Header { label } => update_header_row(&row, label),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowRenderKind {
    Active,
    History,
    Header,
    Unknown,
}

fn row_render_kind(row: &gtk::ListBoxRow) -> RowRenderKind {
    if row.widget_name() == "header" {
        return RowRenderKind::Header;
    }

    let Some(first_child) = row
        .child()
        .and_downcast::<gtk::Box>()
        .and_then(|content| content.first_child())
    else {
        return RowRenderKind::Unknown;
    };

    if first_child.is::<gtk::Stack>() {
        RowRenderKind::Active
    } else if first_child.is::<gtk::Image>() {
        RowRenderKind::History
    } else {
        RowRenderKind::Unknown
    }
}

fn render_state_kind(state: &AgentRowRenderState) -> RowRenderKind {
    match state {
        AgentRowRenderState::Active { .. } => RowRenderKind::Active,
        AgentRowRenderState::History { .. } => RowRenderKind::History,
        AgentRowRenderState::Header { .. } => RowRenderKind::Header,
    }
}

fn replace_row_child(row: &gtk::ListBoxRow, replacement: gtk::ListBoxRow) {
    let Some(child) = replacement.child() else {
        row.set_child(None::<&gtk::Widget>);
        return;
    };
    replacement.set_child(None::<&gtk::Widget>);
    row.set_child(Some(&child));
}

fn active_chat_row_for_state(
    session_id: u64,
    provider_label: &str,
    provider_icon_name: &'static str,
    title: &str,
    state: AgentSessionState,
    usage_label: Option<&str>,
    missing_cli_session_id: bool,
    close_callback: Rc<dyn Fn(u64)>,
) -> gtk::ListBoxRow {
    let icon = gtk::Image::from_icon_name(state_icon_name(provider_icon_name, state));
    icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);
    icon.set_opacity(if matches!(state, AgentSessionState::Active(_)) {
        1.0
    } else {
        0.45
    });
    let spinner = adw::Spinner::new();
    spinner.set_size_request(AGENT_ICON_PIXEL_SIZE, AGENT_ICON_PIXEL_SIZE);
    spinner.set_valign(gtk::Align::Center);

    let icon_stack = gtk::Stack::builder().build();
    icon_stack.add_named(&icon, Some("icon"));
    icon_stack.add_named(&spinner, Some("spinner"));
    icon_stack.set_visible_child_name(
        if state == AgentSessionState::Active(AgentActiveState::Loading) {
            "spinner"
        } else {
            "icon"
        },
    );

    let title_label = title_label(title);
    let meta_text = format!("{provider_label} · {}", state_label(state));
    let meta_label = meta_label(&meta_text);
    let resource_label = active_caption_label(usage_label, missing_cli_session_id);
    let labels = labels_box(&title_label, &meta_label, &resource_label);

    let close_button = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Close session")
        .valign(gtk::Align::Center)
        .build();
    close_button.add_css_class("flat");
    close_button.add_css_class("circular");
    close_button.connect_clicked(move |_| {
        close_callback(session_id);
    });

    let content = row_content();
    content.append(&icon_stack);
    content.append(&labels);
    content.append(&close_button);

    let row = gtk::ListBoxRow::builder().child(&content).build();
    update_active_row(
        &row,
        session_id,
        provider_label,
        provider_icon_name,
        title,
        state,
        usage_label,
        missing_cli_session_id,
    );
    row
}

fn history_chat_row_for_state(
    local_id: i64,
    provider_label: &str,
    provider_icon_name: &'static str,
    title: &str,
    time_label: &str,
    inactive_state: AgentInactiveState,
    restore_state: RestoreState,
) -> gtk::ListBoxRow {
    let icon = gtk::Image::from_icon_name(state_icon_name(
        provider_icon_name,
        AgentSessionState::Inactive(inactive_state),
    ));
    icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);
    icon.set_opacity(HISTORY_AGENT_SESSION_ICON_OPACITY);

    let title_label = title_label(title);
    let meta_text = format!("{provider_label} · {time_label}");
    let meta_label = meta_label(&meta_text);
    let status_text = restore_state_label(restore_state);
    let status_label = gtk::Label::builder()
        .label(status_text)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .width_chars(12)
        .max_width_chars(24)
        .xalign(0.0)
        .visible(!status_text.is_empty())
        .build();
    status_label.add_css_class("dim-label");
    status_label.add_css_class("caption");

    let labels = labels_box(&title_label, &meta_label, &status_label);
    let content = row_content();
    content.append(&icon);
    content.append(&labels);

    let row = gtk::ListBoxRow::builder().child(&content).build();
    update_history_row(
        &row,
        local_id,
        provider_label,
        provider_icon_name,
        title,
        time_label,
        inactive_state,
        restore_state,
    );
    row
}

fn section_header_row(label: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .margin_top(10)
        .margin_bottom(4)
        .margin_start(10)
        .margin_end(10)
        .build();
    label.add_css_class("dim-label");
    label.add_css_class("caption-heading");

    let row = gtk::ListBoxRow::builder()
        .child(&label)
        .activatable(false)
        .selectable(false)
        .build();
    row.set_widget_name("header");
    row
}

fn row_content() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(8)
        .margin_end(8)
        .build()
}

fn labels_box(
    title_label: &gtk::Label,
    meta_label: &gtk::Label,
    resource_label: &gtk::Label,
) -> gtk::Box {
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(1)
        .hexpand(true)
        .build();
    labels.append(title_label);
    labels.append(meta_label);
    labels.append(resource_label);
    labels
}

fn title_label(title: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(title)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .single_line_mode(true)
        .lines(1)
        .width_chars(12)
        .max_width_chars(18)
        .xalign(0.0)
        .hexpand(true)
        .build()
}

fn meta_label(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .width_chars(12)
        .max_width_chars(28)
        .xalign(0.0)
        .build();
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    label.set_tooltip_text(Some(text));
    label
}

fn row_for_identity(list: &gtk::ListBox, identity: RowIdentity) -> Option<gtk::ListBoxRow> {
    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            if row_identity(&row) == Some(identity) {
                return Some(row);
            }
        }
        child = next;
    }
    None
}

fn row_identity(row: &gtk::ListBoxRow) -> Option<RowIdentity> {
    let widget_name = row.widget_name();
    if widget_name == "header" {
        return Some(RowIdentity::Header);
    }
    if let Some(id) = widget_name.strip_prefix("active:") {
        return id.parse().ok().map(RowIdentity::Active);
    }
    if let Some(id) = widget_name.strip_prefix("history:") {
        return id.parse().ok().map(RowIdentity::History);
    }
    None
}

fn update_active_row(
    row: &gtk::ListBoxRow,
    session_id: u64,
    provider_label: &str,
    provider_icon_name: &'static str,
    title: &str,
    state: AgentSessionState,
    usage_label: Option<&str>,
    missing_cli_session_id: bool,
) {
    row.set_widget_name(&format!("active:{session_id}"));
    row.set_tooltip_text(Some(title));
    row.set_selectable(true);
    row.set_activatable(true);
    if let Some(title_label) = row_label_at(row, 0) {
        title_label.set_label(title);
    }
    set_active_row_state(row, provider_label, provider_icon_name, state);
    update_active_row_caption(row, session_id, usage_label, missing_cli_session_id);
}

fn update_history_row(
    row: &gtk::ListBoxRow,
    local_id: i64,
    provider_label: &str,
    provider_icon_name: &'static str,
    title: &str,
    time_label: &str,
    inactive_state: AgentInactiveState,
    restore_state: RestoreState,
) {
    row.set_widget_name(&format!("history:{local_id}"));
    row.set_tooltip_text(Some(title));
    row.set_selectable(restore_state.is_restorable());
    row.set_activatable(restore_state.is_restorable());
    if let Some(icon) = row_leading_image(row) {
        icon.set_icon_name(Some(state_icon_name(
            provider_icon_name,
            AgentSessionState::Inactive(inactive_state),
        )));
        icon.set_opacity(HISTORY_AGENT_SESSION_ICON_OPACITY);
    }
    if let Some(title_label) = row_label_at(row, 0) {
        title_label.set_label(title);
    }
    if let Some(meta_label) = row_label_at(row, 1) {
        let text = format!("{provider_label} · {time_label}");
        meta_label.set_label(&text);
        meta_label.set_tooltip_text(Some(&text));
    }
    if let Some(status_label) = row_label_at(row, 2) {
        let text = restore_state_label(restore_state);
        status_label.set_label(text);
        status_label.set_visible(!text.is_empty());
    }
}

fn update_header_row(row: &gtk::ListBoxRow, label: &str) {
    row.set_widget_name("header");
    row.set_selectable(false);
    row.set_activatable(false);
    if let Some(label_widget) = row.child().and_downcast::<gtk::Label>() {
        label_widget.set_label(label);
    }
}

fn set_active_row_state(
    row: &gtk::ListBoxRow,
    provider_label: &str,
    provider_icon_name: &'static str,
    state: AgentSessionState,
) {
    let Some(content) = row.child().and_downcast::<gtk::Box>() else {
        return;
    };
    let Some(icon_stack) = content.first_child().and_downcast::<gtk::Stack>() else {
        return;
    };
    if state == AgentSessionState::Active(AgentActiveState::Loading) {
        icon_stack.set_visible_child_name("spinner");
    } else {
        if let Some(icon) = icon_stack
            .child_by_name("icon")
            .and_downcast::<gtk::Image>()
        {
            icon.set_icon_name(Some(state_icon_name(provider_icon_name, state)));
            icon.set_opacity(if matches!(state, AgentSessionState::Active(_)) {
                1.0
            } else {
                0.45
            });
        }
        icon_stack.set_visible_child_name("icon");
    }
    if let Some(meta_label) = row_label_at(row, 1) {
        let text = format!("{} · {}", provider_label, state_label(state));
        meta_label.set_label(&text);
        meta_label.set_tooltip_text(Some(&text));
    }
}

fn update_active_row_caption(
    row: &gtk::ListBoxRow,
    session_id: u64,
    usage_label: Option<&str>,
    missing_cli_session_id: bool,
) {
    let Some(caption_label) = row_label_at(row, 2) else {
        return;
    };
    let was_missing = caption_label.is_visible()
        && caption_label.text().as_str() == ACTIVE_MISSING_CLI_SESSION_ID_LABEL;

    if let Some(label) = usage_label {
        caption_label.set_label(label);
        caption_label.set_tooltip_text(Some(label));
        caption_label.set_visible(true);
    } else if missing_cli_session_id {
        caption_label.set_label(ACTIVE_MISSING_CLI_SESSION_ID_LABEL);
        caption_label.set_tooltip_text(Some(ACTIVE_MISSING_CLI_SESSION_ID_TOOLTIP));
        caption_label.set_visible(true);
    } else {
        caption_label.set_label("");
        caption_label.set_tooltip_text(None);
        caption_label.set_visible(false);
    }

    let is_missing = caption_label.is_visible()
        && caption_label.text().as_str() == ACTIVE_MISSING_CLI_SESSION_ID_LABEL;
    if was_missing != is_missing {
        log::debug!(
            "agent list active session id visual changed session_id={} missing_cli_session_id={}",
            session_id,
            is_missing
        );
    }
}

fn active_caption_label(usage_label: Option<&str>, missing_cli_session_id: bool) -> gtk::Label {
    let label_text =
        usage_label.or(missing_cli_session_id.then_some(ACTIVE_MISSING_CLI_SESSION_ID_LABEL));
    let label = gtk::Label::builder()
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .width_chars(12)
        .max_width_chars(24)
        .xalign(0.0)
        .visible(label_text.is_some())
        .build();
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    if let Some(text) = label_text {
        label.set_label(text);
        label.set_tooltip_text(Some(if usage_label.is_some() {
            text
        } else {
            ACTIVE_MISSING_CLI_SESSION_ID_TOOLTIP
        }));
    }
    label
}

fn active_session_missing_cli_session_id(
    session: &ActiveSessionInfo,
    history_rows: &[AgentSessionRow],
) -> bool {
    if session.provider.provider_id() == "codex-app" {
        return false;
    }
    if let Some(row) = active_session_history_row(session, history_rows) {
        return cli_session_id_is_missing(row.cli_session_id.as_deref());
    }

    let Some(local_id) = session.local_history_id else {
        return true;
    };

    match agent_history::lookup_session(local_id) {
        Ok(Some(row)) => cli_session_id_is_missing(row.cli_session_id.as_deref()),
        Ok(None) => {
            log::debug!(
                "agent list active session id visual skipped missing history row session_id={} local_id={}",
                session.session_id,
                local_id
            );
            false
        }
        Err(err) => {
            log::warn!(
                "agent list active session id visual lookup failed session_id={} local_id={} error={}",
                session.session_id,
                local_id,
                err
            );
            false
        }
    }
}

fn active_session_history_row<'a>(
    session: &ActiveSessionInfo,
    history_rows: &'a [AgentSessionRow],
) -> Option<&'a AgentSessionRow> {
    if let Some(local_id) = session.local_history_id {
        if let Some(row) = history_rows.iter().find(|row| row.id == local_id) {
            return Some(row);
        }
    }

    let normalized_title = agent_history::normalize_title(&session.title).to_ascii_lowercase();
    if !agent_history::default_title_should_persist(&normalized_title) {
        return None;
    }

    history_rows.iter().find(|row| {
        row.provider_id == session.provider.provider_id()
            && row.normalized_title == normalized_title
    })
}

fn cli_session_id_is_missing(cli_session_id: Option<&str>) -> bool {
    cli_session_id.unwrap_or("").trim().is_empty()
}

fn row_label_at(row: &gtk::ListBoxRow, index: usize) -> Option<gtk::Label> {
    let labels = row
        .child()
        .and_downcast::<gtk::Box>()?
        .first_child()?
        .next_sibling()?
        .downcast::<gtk::Box>()
        .ok()?;
    let mut child = labels.first_child()?;
    for _ in 0..index {
        child = child.next_sibling()?;
    }
    child.downcast::<gtk::Label>().ok()
}

fn row_leading_image(row: &gtk::ListBoxRow) -> Option<gtk::Image> {
    row.child()?
        .downcast::<gtk::Box>()
        .ok()?
        .first_child()?
        .downcast::<gtk::Image>()
        .ok()
}

fn state_icon_name(provider_icon_name: &'static str, state: AgentSessionState) -> &'static str {
    match state {
        AgentSessionState::Active(AgentActiveState::Asking) => WAITING_AGENT_SESSION_ICON,
        AgentSessionState::Active(
            AgentActiveState::NewChat | AgentActiveState::Idle | AgentActiveState::Loading,
        ) => provider_icon_name,
        AgentSessionState::Inactive(AgentInactiveState::Unloaded) => provider_icon_name,
        AgentSessionState::Inactive(AgentInactiveState::Dead) => UNRESTORABLE_AGENT_SESSION_ICON,
    }
}

fn active_history_keys(active_sessions: &[ActiveSessionInfo]) -> std::collections::HashSet<String> {
    active_sessions
        .iter()
        .flat_map(|session| {
            let mut keys = Vec::new();
            if let Some(local_id) = session.local_history_id {
                keys.push(format!("id:{local_id}"));
            }
            let title = agent_history::normalize_title(&session.title).to_ascii_lowercase();
            if agent_history::default_title_should_persist(&title) {
                keys.push(format!("title:{}:{title}", session.provider.provider_id()));
            }
            keys
        })
        .collect()
}

fn normalize_search_query(query: &str) -> String {
    query.trim().to_lowercase()
}

fn agent_title_matches_query(title: &str, query: &str) -> bool {
    query.is_empty() || title.to_lowercase().contains(query)
}

fn state_label(state: AgentSessionState) -> &'static str {
    match state {
        AgentSessionState::Active(AgentActiveState::NewChat) => "New Chat",
        AgentSessionState::Active(AgentActiveState::Idle) => "Idle",
        AgentSessionState::Active(AgentActiveState::Loading) => "Loading",
        AgentSessionState::Active(AgentActiveState::Asking) => "Asking",
        AgentSessionState::Inactive(AgentInactiveState::Unloaded) => "Unloaded",
        AgentSessionState::Inactive(AgentInactiveState::Dead) => "Dead",
    }
}

fn history_inactive_state(restore_state: RestoreState) -> AgentInactiveState {
    if restore_state.is_restorable() {
        AgentInactiveState::Unloaded
    } else {
        AgentInactiveState::Dead
    }
}

fn provider_icon_name(provider_id: &str) -> &'static str {
    provider::all_providers()
        .iter()
        .copied()
        .find(|provider| provider.provider_id() == provider_id)
        .map(|provider| provider.session_icon_name())
        .unwrap_or("brain-augemnted-symbolic")
}

fn provider_label(provider_id: &str) -> &'static str {
    provider::all_providers()
        .iter()
        .copied()
        .find(|provider| provider.provider_id() == provider_id)
        .map(|provider| provider.label())
        .unwrap_or("Agent")
}

fn restore_state_label(state: RestoreState) -> &'static str {
    match state {
        RestoreState::Unmapped => "Not restorable yet",
        RestoreState::Restorable => "",
        RestoreState::Unsupported => "Restore unsupported",
        RestoreState::Ambiguous => "Restore ambiguous",
        RestoreState::Missing => "Restore unavailable",
    }
}

fn history_monitor_event_should_reload(
    file: &gio::File,
    other_file: Option<&gio::File>,
    event_type: gio::FileMonitorEvent,
) -> bool {
    if matches!(
        event_type,
        gio::FileMonitorEvent::AttributeChanged
            | gio::FileMonitorEvent::PreUnmount
            | gio::FileMonitorEvent::Unmounted
    ) {
        return false;
    }

    [file.path(), other_file.and_then(|file| file.path())]
        .into_iter()
        .flatten()
        .any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, "sessions.sqlite" | "sessions.sqlite-wal"))
        })
}

fn history_db_signature(craic_dir: &std::path::Path) -> HistoryDbSignature {
    let database = history_db_file_signature(&craic_dir.join("sessions.sqlite"), false);
    let wal = history_db_file_signature(&craic_dir.join("sessions.sqlite-wal"), true);
    HistoryDbSignature { database, wal }
}

fn history_db_file_signature(
    path: &std::path::Path,
    ignore_empty: bool,
) -> Option<HistoryDbFileSignature> {
    let metadata = std::fs::metadata(path).ok()?;
    if ignore_empty && metadata.len() == 0 {
        return None;
    }
    Some(HistoryDbFileSignature {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn adjustment_is_near_bottom(adjustment: &gtk::Adjustment) -> bool {
    adjustment.value() + adjustment.page_size() + HISTORY_PREFETCH_DISTANCE >= adjustment.upper()
}

fn history_group_label(ms: i64) -> String {
    let age = agent_history::unix_now_ms().saturating_sub(ms);
    let day = 24 * 60 * 60 * 1000;
    if age < day {
        return "Today".to_string();
    }
    if age < 2 * day {
        return "Yesterday".to_string();
    }
    if age < 7 * day {
        return "Last 7 Days".to_string();
    }
    if age < 30 * day {
        return "Last 30 Days".to_string();
    }
    glib::DateTime::from_unix_local(ms / 1000)
        .and_then(|time| time.format("%B %Y"))
        .map(|label| label.to_string())
        .unwrap_or_else(|_| "Older".to_string())
}

fn history_time_label(ms: i64) -> String {
    glib::DateTime::from_unix_local(ms / 1000)
        .and_then(|time| time.format("%b %d, %I:%M %p"))
        .map(|label| label.to_string())
        .unwrap_or_else(|_| "recently".to_string())
}
