use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

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
const HISTORY_AGENT_SESSION_ICON_OPACITY: f64 = 0.58;
const WAITING_AGENT_SESSION_ICON: &str = "hand-touch-symbolic";
const UNRESTORABLE_AGENT_SESSION_ICON: &str = "background-app-ghost-symbolic";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentListSelection {
    Active(u64),
    History(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentLaunch {
    Grok,
    OpenCode,
    CodexCli,
    Agy,
}

impl AgentLaunch {
    pub fn terminal_provider(self) -> &'static dyn AgentProvider {
        match self {
            AgentLaunch::Grok => &provider::grok::PROVIDER,
            AgentLaunch::OpenCode => &provider::opencode::PROVIDER,
            AgentLaunch::CodexCli => &provider::codex::PROVIDER,
            AgentLaunch::Agy => &provider::agy::PROVIDER,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentListContextAction {
    ViewStatusHistory(i64),
    ViewStatusActive(u64),
    GenerateSummaryHistory(i64),
    GenerateSummaryActive(u64),
    Unload(i64),
    Hide(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentListContextTarget {
    session_id: Option<u64>,
    local_id: Option<i64>,
    loaded: bool,
    has_summary: bool,
    can_summarize: bool,
}

#[derive(Clone)]
pub struct AgentList {
    pub root: gtk::Box,
    grok_button: gtk::Button,
    opencode_button: gtk::Button,
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
    native_history_enabled: Rc<Cell<bool>>,
    history_warnings: Rc<RefCell<Vec<String>>>,
    history_rows: Rc<RefCell<Vec<AgentSessionRow>>>,
    search_query: Rc<RefCell<String>>,
    selected_tags: Rc<RefCell<HashSet<String>>>,
    row_widgets: Rc<RefCell<HashMap<AgentRowKey, gtk::ListBoxRow>>>,
    row_states: Rc<RefCell<HashMap<AgentRowKey, AgentRowRenderState>>>,
    loaded_limit: Rc<Cell<usize>>,
    has_more: Rc<Cell<bool>>,
    loading: Rc<Cell<bool>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
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
    tags: Option<Vec<WorkspaceTag>>,
    warnings: Vec<String>,
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
    if target.can_summarize {
        session_items.push(ActionMenuItem::new(
            summary_label,
            summary_action,
            target.loaded,
        ));
    }
    if let Some(local_id) = target.local_id {
        session_items.push(ActionMenuItem::new(
            "Unload Session",
            AgentListContextAction::Unload(local_id),
            target.loaded,
        ));
        session_items.push(ActionMenuItem::new(
            "Hide from History",
            AgentListContextAction::Hide(local_id),
            true,
        ));
    }

    vec![ActionMenuSection::new(session_items)]
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

fn active_row_render_state(session: &ActiveSessionInfo) -> AgentRowRenderState {
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
        } => active_chat_row_for_state(
            *session_id,
            provider_label,
            provider_icon_name,
            title,
            *state,
            usage_label.as_deref(),
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
        } => update_active_row(
            &row,
            *session_id,
            provider_label,
            provider_icon_name,
            title,
            *state,
            usage_label.as_deref(),
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
    let resource_label = active_caption_label(usage_label);
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
) {
    row.set_widget_name(&format!("active:{session_id}"));
    row.set_tooltip_text(Some(title));
    row.set_selectable(true);
    row.set_activatable(true);
    if let Some(title_label) = row_label_at(row, 0) {
        title_label.set_label(title);
    }
    set_active_row_state(row, provider_label, provider_icon_name, state);
    update_active_row_caption(row, usage_label);
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

fn update_active_row_caption(row: &gtk::ListBoxRow, usage_label: Option<&str>) {
    if let Some(caption) = row_label_at(row, 2) {
        caption.set_label(usage_label.unwrap_or_default());
        caption.set_tooltip_text(usage_label);
        caption.set_visible(usage_label.is_some());
    }
}

fn active_caption_label(usage_label: Option<&str>) -> gtk::Label {
    let label_text = usage_label;
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
        label.set_tooltip_text(Some(text));
    }
    label
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

fn active_history_keys(active_sessions: &[ActiveSessionInfo]) -> HashSet<String> {
    active_sessions
        .iter()
        .filter_map(|session| session.local_history_id)
        .map(|id| format!("id:{id}"))
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
