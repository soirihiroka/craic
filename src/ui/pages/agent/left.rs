use adw::prelude::*;
use craic_diff_ui::{Element, PartialEqRenderState};
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use crate::ui::agent_history::{self, AgentSessionRow, RestoreState, WorkspaceKey};
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
pub(in crate::ui) enum AgentListSelection {
    Active(u64),
    History(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum AgentListContextAction {
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
}

#[derive(Clone)]
pub(in crate::ui) struct AgentList {
    pub(in crate::ui) root: gtk::Box,
    codex_button: gtk::Button,
    agy_button: gtk::Button,
    opencode_button: gtk::Button,
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
    row_reconciler:
        Rc<RefCell<craic_diff_ui::gtk::ListBoxReconciler<AgentRowKey, AgentRowRenderState>>>,
    loaded_limit: Rc<Cell<usize>>,
    has_more: Rc<Cell<bool>>,
    loading: Rc<Cell<bool>>,
    history_monitor: Rc<RefCell<Option<gio::FileMonitor>>>,
    debounce_source: Rc<RefCell<Option<glib::SourceId>>>,
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

impl AgentList {
    pub(in crate::ui) fn new() -> Self {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("navigation-sidebar");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
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
            "agent_list",
        );

        let providers = provider::all_providers();
        let codex_button = new_agent_button(providers[0]);
        let agy_button = new_agent_button(providers[1]);
        let opencode_button = new_agent_button(providers[2]);

        let search_panel = SearchPanel::new("Search agents");
        search_panel.set_options_visible(false);
        search_panel.set_navigation_visible(false);

        let bottom_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::Center)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        bottom_bar.append(&codex_button);
        bottom_bar.append(&agy_button);
        bottom_bar.append(&opencode_button);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        root.append(&search_panel.widget());
        root.append(&scroller_overlay);
        root.append(&bottom_bar);

        let agent_list = Self {
            root,
            codex_button,
            agy_button,
            opencode_button,
            search_panel,
            list,
            scroller,
            suppress_selection_callback: Rc::new(Cell::new(false)),
            selection_callback: Rc::new(RefCell::new(None)),
            context_action_callback: Rc::new(RefCell::new(None)),
            close_callback: Rc::new(RefCell::new(None)),
            active_sessions: Rc::new(RefCell::new(Vec::new())),
            workspace: Rc::new(RefCell::new(None)),
            history_rows: Rc::new(RefCell::new(Vec::new())),
            search_query: Rc::new(RefCell::new(String::new())),
            selected_tags: Rc::new(RefCell::new(HashSet::new())),
            row_reconciler: Rc::new(RefCell::new(craic_diff_ui::gtk::ListBoxReconciler::new())),
            loaded_limit: Rc::new(Cell::new(HISTORY_PAGE_SIZE)),
            has_more: Rc::new(Cell::new(false)),
            loading: Rc::new(Cell::new(false)),
            history_monitor: Rc::new(RefCell::new(None)),
            debounce_source: Rc::new(RefCell::new(None)),
            active_context_menu: Rc::new(RefCell::new(None)),
        };
        agent_list.connect_search();
        agent_list
            .search_panel
            .set_key_capture_widget(&agent_list.root);
        agent_list.install_search_shortcuts(&agent_list.root);
        agent_list.install_search_shortcuts(&agent_list.list);
        agent_list.install_search_shortcuts(&agent_list.scroller);
        agent_list.install_search_shortcuts(&agent_list.codex_button);
        agent_list.install_search_shortcuts(&agent_list.agy_button);
        agent_list.install_search_shortcuts(&agent_list.opencode_button);
        agent_list.connect_selection();
        agent_list.connect_context_menu();
        agent_list.connect_auto_paging();
        agent_list.restart_history_monitor();
        agent_list
    }

    pub(in crate::ui) fn set_workspace_key(&self, workspace_key: String, target_root: String) {
        let next = agent_history::workspace_for_system_path(workspace_key, target_root);
        if self
            .workspace
            .borrow()
            .as_ref()
            .is_some_and(|workspace| workspace.key() == next.key())
        {
            return;
        }

        log::info!(
            "agent history workspace changed key={} target_root={}",
            next.key(),
            next.repo_path().display()
        );
        self.workspace.replace(Some(next));
        self.loaded_limit.set(HISTORY_PAGE_SIZE);
        self.has_more.set(false);
        self.selected_tags.borrow_mut().clear();
        self.reload_tags();
        self.reload_history();
    }

    pub(in crate::ui) fn install_search_shortcuts<W: IsA<gtk::Widget>>(&self, widget: &W) {
        self.search_panel.install_shortcuts(widget);
    }

    pub(in crate::ui) fn toggle_search(&self) {
        self.search_panel.toggle();
    }

    pub(in crate::ui) fn reload_history(&self) {
        if self.loading.replace(true) {
            return;
        }

        let search_query = self.search_query.borrow().clone();
        let selected_tags = sorted_tags(&self.selected_tags.borrow());
        let rows = self
            .workspace
            .borrow()
            .as_ref()
            .map(|workspace| {
                agent_history::list_sessions(
                    workspace.key(),
                    self.loaded_limit.get().saturating_add(1),
                    0,
                    Some(&search_query),
                    &selected_tags,
                )
            })
            .unwrap_or_else(|| Ok(Vec::new()));

        self.loading.set(false);
        match rows {
            Ok(mut rows) => {
                let has_more = rows.len() > self.loaded_limit.get();
                if has_more {
                    rows.truncate(self.loaded_limit.get());
                }
                self.has_more.set(has_more);
                self.history_rows.replace(rows);
                self.reconcile_rows();
            }
            Err(err) => {
                log::warn!("agent history load failed: {err}");
                self.has_more.set(false);
                self.history_rows.replace(Vec::new());
                self.reconcile_rows();
            }
        }
    }

    pub(in crate::ui) fn reload_workspace_history(&self) {
        log::debug!("agent workspace history refresh requested");
        self.reload_tags();
        self.reload_history();
    }

    pub(in crate::ui) fn connect_new_chat<F>(&self, callback: F)
    where
        F: Fn(&'static dyn AgentProvider) + 'static,
    {
        let callback = Rc::new(callback);
        let providers = provider::all_providers();

        self.codex_button.connect_clicked({
            let callback = callback.clone();

            move |_| {
                callback(providers[0]);
            }
        });
        self.agy_button.connect_clicked({
            let callback = callback.clone();

            move |_| {
                callback(providers[1]);
            }
        });
        self.opencode_button.connect_clicked(move |_| {
            callback(providers[2]);
        });
    }

    pub(in crate::ui) fn add_session_row(
        &self,
        session_id: u64,
        provider: &'static dyn AgentProvider,
        title: &str,
        local_history_id: Option<i64>,
        state: AgentSessionState,
    ) {
        if self
            .active_sessions
            .borrow()
            .iter()
            .any(|session| session.session_id == session_id)
        {
            return;
        }

        let last_seen_at_ms = self.active_session_last_seen_at_ms(local_history_id);
        if let Some(local_id) = local_history_id {
            log::debug!(
                "agent list restored row placed by history timestamp session_id={} local_id={} last_seen_at_ms={}",
                session_id,
                local_id,
                last_seen_at_ms
            );
        }

        self.active_sessions.borrow_mut().insert(
            0,
            ActiveSessionInfo {
                session_id,
                provider,
                title: title.to_string(),
                local_history_id,
                state,
                usage: None,
                last_seen_at_ms,
            },
        );
        self.reconcile_rows();
        self.select_session(session_id);
    }

    pub(in crate::ui) fn select_session(&self, session_id: u64) {
        if let Some(row) = row_for_identity(&self.list, RowIdentity::Active(session_id)) {
            self.select_row_without_callback(&row);
        }
    }

    pub(in crate::ui) fn connect_selected<F>(&self, callback: F)
    where
        F: Fn(AgentListSelection) + 'static,
    {
        self.selection_callback.replace(Some(Rc::new(callback)));
    }

    pub(in crate::ui) fn connect_context_action<F>(&self, callback: F)
    where
        F: Fn(AgentListContextAction) + 'static,
    {
        self.context_action_callback
            .replace(Some(Rc::new(callback)));
    }

    pub(in crate::ui) fn connect_close_requested<F>(&self, callback: F)
    where
        F: Fn(u64) + 'static,
    {
        self.close_callback.replace(Some(Rc::new(callback)));
    }

    pub(in crate::ui) fn remove_session(&self, session_id: u64) -> bool {
        let before = self.active_sessions.borrow().len();
        self.active_sessions
            .borrow_mut()
            .retain(|session| session.session_id != session_id);
        self.reconcile_rows();
        before != self.active_sessions.borrow().len()
    }

    pub(in crate::ui) fn update_title(&self, session_id: u64, title: &str) {
        let mut changed = false;
        if let Some(session) = self
            .active_sessions
            .borrow_mut()
            .iter_mut()
            .find(|session| session.session_id == session_id)
        {
            if session.title == title {
                return;
            }
            session.title = title.to_string();
            if session.local_history_id.is_none() {
                session.last_seen_at_ms = agent_history::unix_now_ms();
            }
            changed = true;
        }
        if changed {
            self.reconcile_rows();
        }
    }

    pub(in crate::ui) fn set_session_state(
        &self,
        session_id: u64,
        provider: &'static dyn AgentProvider,
        state: AgentSessionState,
    ) -> bool {
        let mut changed = false;
        if let Some(session) = self
            .active_sessions
            .borrow_mut()
            .iter_mut()
            .find(|session| session.session_id == session_id)
        {
            if session.provider.provider_id() != provider.provider_id() || session.state != state {
                session.provider = provider;
                session.state = state;
                if !matches!(state, AgentSessionState::Active(_)) {
                    session.usage = None;
                }
                changed = true;
            }
        }
        if changed {
            self.reconcile_rows();
        }
        changed
    }

    pub(in crate::ui) fn set_resource_usage(
        &self,
        session_id: u64,
        usage: Option<AgentResourceUsage>,
    ) {
        let mut changed = false;
        if let Some(session) = self
            .active_sessions
            .borrow_mut()
            .iter_mut()
            .find(|session| session.session_id == session_id)
        {
            if session.usage == usage {
                return;
            }
            session.usage = usage;
            changed = true;
        }
        if changed {
            self.reconcile_rows();
        }
    }

    fn connect_selection(&self) {
        self.list.connect_row_selected({
            let suppress_selection_callback = self.suppress_selection_callback.clone();
            let selection_callback = self.selection_callback.clone();
            let history_rows = self.history_rows.clone();
            let list = self.list.clone();

            move |_, row| {
                if suppress_selection_callback.get() {
                    return;
                }
                let Some(row) = row else {
                    return;
                };
                match row_identity(row) {
                    Some(RowIdentity::Active(session_id)) => {
                        if let Some(ref cb) = *selection_callback.borrow() {
                            cb(AgentListSelection::Active(session_id));
                        }
                    }
                    Some(RowIdentity::History(local_id)) => {
                        let restorable = history_rows
                            .borrow()
                            .iter()
                            .find(|row| row.id == local_id)
                            .is_some_and(|row| row.restore_state.is_restorable());
                        if restorable {
                            if let Some(ref cb) = *selection_callback.borrow() {
                                cb(AgentListSelection::History(local_id));
                            }
                        } else {
                            log::info!(
                                "agent history inactive row selected but not restorable local_id={}",
                                local_id
                            );
                            list.unselect_row(row);
                        }
                    }
                    Some(RowIdentity::Header) | None => {
                        list.unselect_row(row);
                    }
                }
            }
        });
    }

    fn connect_search(&self) {
        self.search_panel.connect_query_changed({
            let agent_list = self.clone();

            move |query| {
                agent_list.update_search_query(query.trim().to_string());
            }
        });

        self.search_panel.connect_closed({
            let agent_list = self.clone();

            move || {
                agent_list.clear_search_filters();
            }
        });

        self.search_panel.connect_tag_toggled({
            let agent_list = self.clone();

            move |tag, active| agent_list.update_selected_tag(tag, active)
        });
    }

    fn update_search_query(&self, query: String) {
        let query = normalize_search_query(&query);
        if *self.search_query.borrow() == query {
            return;
        }

        self.search_query.replace(query.clone());
        self.loaded_limit.set(HISTORY_PAGE_SIZE);
        self.has_more.set(false);
        log::debug!(
            "agent list search updated query_len={} history_limit={}",
            query.len(),
            self.loaded_limit.get()
        );
        self.reload_history();
    }

    fn update_selected_tag(&self, tag: String, active: bool) {
        let changed = if active {
            self.selected_tags.borrow_mut().insert(tag.clone())
        } else {
            self.selected_tags.borrow_mut().remove(&tag)
        };
        if !changed {
            return;
        }

        let selected = sorted_tags(&self.selected_tags.borrow());
        log::debug!(
            "agent list tag filter updated count={} tags={:?}",
            selected.len(),
            selected
        );
        self.loaded_limit.set(HISTORY_PAGE_SIZE);
        self.has_more.set(false);
        self.reload_tags();
        self.reload_history();
    }

    fn clear_search_filters(&self) {
        let query_changed = !self.search_query.borrow().is_empty();
        let tags_changed = !self.selected_tags.borrow().is_empty();
        if !query_changed && !tags_changed {
            return;
        }

        self.search_query.replace(String::new());
        self.selected_tags.borrow_mut().clear();
        self.loaded_limit.set(HISTORY_PAGE_SIZE);
        self.has_more.set(false);
        log::debug!(
            "agent list search cleared query_changed={} tag_count=0",
            query_changed
        );
        self.reload_tags();
        self.reload_history();
    }

    fn connect_context_menu(&self) {
        let click = gtk::GestureClick::builder().button(0).build();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed({
            let agent_list = self.clone();

            move |gesture, _, x, y| {
                if gesture.current_button() != 3 {
                    return;
                }

                let Some(row) = agent_list.list.row_at_y(y as i32) else {
                    return;
                };
                let Some(target) = agent_list.context_target_for_row(&row) else {
                    log::debug!("agent session context menu skipped for row without history id");
                    return;
                };

                agent_list.select_row_without_callback(&row);
                show_agent_session_context_menu(&agent_list, target, x, y);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.list.add_controller(click);
    }

    fn connect_auto_paging(&self) {
        self.scroller.connect_edge_reached({
            let agent_list = self.clone();

            move |_, position| {
                if position == gtk::PositionType::Bottom {
                    agent_list.load_more_history();
                }
            }
        });

        self.scroller.vadjustment().connect_value_changed({
            let agent_list = self.clone();

            move |adjustment| {
                if adjustment_is_near_bottom(adjustment) {
                    agent_list.load_more_history();
                }
            }
        });
    }

    fn load_more_history(&self) {
        if self.loading.get() || !self.has_more.get() {
            return;
        }
        self.loaded_limit
            .set(self.loaded_limit.get().saturating_add(HISTORY_PAGE_SIZE));
        self.reload_history();
    }

    fn restart_history_monitor(&self) {
        if let Some(source_id) = self.debounce_source.borrow_mut().take() {
            source_id.remove();
        }
        if let Some(monitor) = self.history_monitor.borrow_mut().take() {
            monitor.cancel();
        }

        let Some(craic_dir) = crate::config::craic_dir() else {
            log::debug!("agent history monitor skipped because HOME is not set");
            return;
        };
        if let Err(err) = std::fs::create_dir_all(&craic_dir) {
            log::warn!(
                "agent history monitor could not create {}: {err}",
                craic_dir.display()
            );
            return;
        }

        let file = gio::File::for_path(&craic_dir);
        let flags = gio::FileMonitorFlags::WATCH_MOVES | gio::FileMonitorFlags::SEND_MOVED;
        let monitor = match file.monitor_directory(flags, None::<&gio::Cancellable>) {
            Ok(monitor) => monitor,
            Err(err) => {
                log::warn!(
                    "agent history monitor failed for {}: {err}",
                    craic_dir.display()
                );
                return;
            }
        };
        monitor.set_rate_limit(HISTORY_DB_MONITOR_RATE_LIMIT_MS);
        monitor.connect_changed({
            let agent_list = self.clone();

            move |_, file, other_file, event_type| {
                if history_monitor_event_should_reload(file, other_file, event_type) {
                    agent_list.queue_history_reload();
                }
            }
        });
        self.history_monitor.replace(Some(monitor));
    }

    fn queue_history_reload(&self) {
        if self.debounce_source.borrow().is_some() {
            return;
        }

        let agent_list = self.clone();
        let source_id = glib::timeout_add_local(HISTORY_DB_REFRESH_DEBOUNCE, move || {
            agent_list.debounce_source.borrow_mut().take();
            agent_list.reload_tags();
            agent_list.reload_history();
            glib::ControlFlow::Break
        });
        self.debounce_source.replace(Some(source_id));
    }

    fn reload_tags(&self) {
        let tags = self
            .workspace
            .borrow()
            .as_ref()
            .map(|workspace| agent_history::workspace_tag_counts(workspace.key()))
            .unwrap_or_else(|| Ok(Vec::new()))
            .unwrap_or_else(|err| {
                log::warn!("agent history tags load failed: {err}");
                Vec::new()
            });
        let selected_tags = self.selected_tags.borrow();
        let tags = tags
            .into_iter()
            .map(|workspace_tag| SearchTag {
                active: selected_tags.contains(&workspace_tag.tag),
                id: workspace_tag.tag.clone(),
                label: workspace_tag.tag,
                count: Some(workspace_tag.session_count),
            })
            .collect::<Vec<_>>();
        self.search_panel.set_tags(tags);
    }

    fn reconcile_rows(&self) {
        let selected = self.list.selected_row().and_then(|row| row_identity(&row));
        let search_query = self.search_query.borrow().clone();
        let tag_filter_active = !self.selected_tags.borrow().is_empty();
        let history_filter_active = tag_filter_active || !search_query.is_empty();

        let close_callback_holder = self.close_callback.clone();
        let close_cb = Rc::new(move |sid| {
            if let Some(ref cb) = *close_callback_holder.borrow() {
                cb(sid);
            }
        });

        let active_sessions = self.active_sessions.borrow().clone();
        let history_rows = self.history_rows.borrow();
        let mut elements = Vec::new();
        let mut pinned_active_sessions = active_sessions
            .iter()
            .filter(|session| session.local_history_id.is_none())
            .filter(|_| !tag_filter_active)
            .filter(|session| agent_title_matches_query(&session.title, &search_query))
            .collect::<Vec<_>>();
        pinned_active_sessions.sort_by(|left, right| {
            right
                .last_seen_at_ms
                .cmp(&left.last_seen_at_ms)
                .then_with(|| right.session_id.cmp(&left.session_id))
        });
        for session in pinned_active_sessions {
            elements.push(Element::new(
                AgentRowKey::Active(session.session_id),
                active_row_render_state(session, history_rows.as_slice()),
            ));
        }

        let active_keys = active_history_keys(&active_sessions);
        let mut current_group = String::new();
        let mut timeline_rows = active_sessions
            .iter()
            .filter(|session| session.local_history_id.is_some())
            .filter(|_| !history_filter_active)
            .filter(|session| agent_title_matches_query(&session.title, &search_query))
            .map(TimelineRow::Active)
            .collect::<Vec<_>>();
        for row in history_rows.iter() {
            if !history_filter_active
                && (active_keys.contains(&format!("id:{}", row.id))
                    || active_keys.contains(&format!(
                        "title:{}:{}",
                        row.provider_id, row.normalized_title
                    )))
            {
                continue;
            }
            timeline_rows.push(TimelineRow::History(row));
        }

        timeline_rows.sort_by(|left, right| {
            right
                .last_seen_at_ms()
                .cmp(&left.last_seen_at_ms())
                .then_with(|| right.identity_order().cmp(&left.identity_order()))
        });

        for row in timeline_rows {
            let group = history_group_label(row.last_seen_at_ms());
            if group != current_group {
                current_group = group.clone();
                elements.push(Element::new(
                    AgentRowKey::Header(group.clone()),
                    AgentRowRenderState::Header { label: group },
                ));
            }
            match row {
                TimelineRow::Active(session) => {
                    elements.push(Element::new(
                        AgentRowKey::Active(session.session_id),
                        active_row_render_state(session, history_rows.as_slice()),
                    ));
                }
                TimelineRow::History(row) => {
                    elements.push(Element::new(
                        AgentRowKey::History(row.id),
                        history_row_render_state(row),
                    ));
                }
            }
        }

        let mount_close_cb = close_cb.clone();
        let update_close_cb = close_cb.clone();
        let _ = self.row_reconciler.borrow_mut().reconcile(
            &self.list,
            elements,
            PartialEqRenderState,
            move |_, _, state| agent_row(state, mount_close_cb.clone()).upcast::<gtk::Widget>(),
            move |_, widget, _, next| update_agent_row(widget, next, update_close_cb.clone()),
        );

        if let Some(selected) = selected.and_then(|identity| row_for_identity(&self.list, identity))
        {
            self.select_row_without_callback(&selected);
        }
    }

    fn select_row_without_callback(&self, row: &gtk::ListBoxRow) {
        self.suppress_selection_callback.set(true);
        self.list.select_row(Some(row));
        self.suppress_selection_callback.set(false);
    }

    fn active_session_last_seen_at_ms(&self, local_history_id: Option<i64>) -> i64 {
        let Some(local_id) = local_history_id else {
            return agent_history::unix_now_ms();
        };

        if let Some(last_seen_at_ms) = self
            .history_rows
            .borrow()
            .iter()
            .find(|row| row.id == local_id)
            .map(|row| row.last_seen_at_ms)
        {
            return last_seen_at_ms;
        }

        match agent_history::lookup_session(local_id) {
            Ok(Some(row)) => row.last_seen_at_ms,
            Ok(None) => {
                log::warn!(
                    "agent list could not find restored history timestamp local_id={}",
                    local_id
                );
                agent_history::unix_now_ms()
            }
            Err(err) => {
                log::warn!(
                    "agent list failed to load restored history timestamp local_id={} error={}",
                    local_id,
                    err
                );
                agent_history::unix_now_ms()
            }
        }
    }

    fn context_target_for_row(&self, row: &gtk::ListBoxRow) -> Option<AgentListContextTarget> {
        match row_identity(row)? {
            RowIdentity::History(local_id) => Some(AgentListContextTarget {
                session_id: None,
                local_id: Some(local_id),
                loaded: false,
                has_summary: self.history_row_has_summary(local_id),
            }),
            RowIdentity::Active(session_id) => self
                .active_sessions
                .borrow()
                .iter()
                .find(|session| session.session_id == session_id)
                .map(|session| AgentListContextTarget {
                    session_id: Some(session.session_id),
                    local_id: session.local_history_id,
                    loaded: true,
                    has_summary: session
                        .local_history_id
                        .is_some_and(|local_id| self.history_row_has_summary(local_id)),
                }),
            RowIdentity::Header => None,
        }
    }

    fn history_row_has_summary(&self, local_id: i64) -> bool {
        self.history_rows
            .borrow()
            .iter()
            .find(|row| row.id == local_id)
            .is_some_and(|row| row.task_description.is_some())
    }
}

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

    let mut session_items = vec![
        ActionMenuItem::new("View Status", view_action, true),
        ActionMenuItem::new(summary_label, summary_action, target.loaded),
        ActionMenuItem::new("Set Session ID...", set_session_id_action, true),
    ];
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

fn new_agent_button(provider: &'static dyn AgentProvider) -> gtk::Button {
    let icon = gtk::Image::from_icon_name("list-add-symbolic");
    icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);

    let label = gtk::Label::new(Some(provider.label()));

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .build();
    content.append(&icon);
    content.append(&label);

    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text(format!("New {} chat", provider.label()))
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
        gio::FileMonitorEvent::PreUnmount | gio::FileMonitorEvent::Unmounted
    ) {
        return false;
    }

    [file.path(), other_file.and_then(|file| file.path())]
        .into_iter()
        .flatten()
        .any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    matches!(
                        name,
                        "sessions.sqlite" | "sessions.sqlite-wal" | "sessions.sqlite-shm"
                    )
                })
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
