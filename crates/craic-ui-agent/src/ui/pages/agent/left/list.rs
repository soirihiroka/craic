impl AgentList {
    pub fn new() -> Self {
        if let Err(err) = agent_history::initialize_history_database() {
            log::warn!("agent history database initialization failed: {err}");
        }
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

        let app_button = new_agent_button("App");
        let codex_cli_button = new_agent_button(provider::codex::PROVIDER.label());
        let agy_button = new_agent_button(provider::agy::PROVIDER.label());

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
        bottom_bar.append(&app_button);
        bottom_bar.append(&codex_cli_button);
        bottom_bar.append(&agy_button);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        root.append(&search_panel.widget());
        root.append(&scroller_overlay);
        root.append(&bottom_bar);

        let agent_list = Self {
            root,
            app_button,
            codex_cli_button,
            agy_button,
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
            row_widgets: Rc::new(RefCell::new(HashMap::new())),
            row_states: Rc::new(RefCell::new(HashMap::new())),
            loaded_limit: Rc::new(Cell::new(HISTORY_PAGE_SIZE)),
            has_more: Rc::new(Cell::new(false)),
            loading: Rc::new(Cell::new(false)),
            history_monitor: Rc::new(RefCell::new(None)),
            debounce_source: Rc::new(RefCell::new(None)),
            history_db_signature: Rc::new(RefCell::new(HistoryDbSignature::default())),
            history_monitor_stats: Rc::new(RefCell::new(HistoryMonitorStats::default())),
            active_context_menu: Rc::new(RefCell::new(None)),
        };
        agent_list.connect_search();
        agent_list
            .search_panel
            .set_key_capture_widget(&agent_list.root);
        agent_list.install_search_shortcuts(&agent_list.root);
        agent_list.install_search_shortcuts(&agent_list.list);
        agent_list.install_search_shortcuts(&agent_list.scroller);
        agent_list.install_search_shortcuts(&agent_list.app_button);
        agent_list.install_search_shortcuts(&agent_list.codex_cli_button);
        agent_list.install_search_shortcuts(&agent_list.agy_button);
        agent_list.connect_selection();
        agent_list.connect_context_menu();
        agent_list.connect_auto_paging();
        agent_list.restart_history_monitor();
        agent_list
    }

    pub fn set_workspace_key(&self, workspace_key: String, target_root: String) {
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
        self.history_rows.borrow_mut().clear();
        self.search_panel.set_tags(Vec::new());
        self.apply_rows();
        self.reload_workspace_history();
    }

    pub fn install_search_shortcuts<W: IsA<gtk::Widget>>(&self, widget: &W) {
        self.search_panel.install_shortcuts(widget);
    }

    pub fn toggle_search(&self) {
        self.search_panel.toggle();
    }

    pub fn reload_history(&self) {
        if self.loading.replace(true) {
            return;
        }

        self.acknowledge_history_db_signature();
        self.spawn_history_load(false);
    }

    pub fn reload_workspace_history(&self) {
        if self.loading.replace(true) {
            return;
        }

        self.acknowledge_history_db_signature();
        self.spawn_history_load(true);
    }

    fn spawn_history_load(&self, load_tags: bool) {
        let Some(workspace) = self.workspace.borrow().clone() else {
            self.loading.set(false);
            self.history_rows.borrow_mut().clear();
            self.has_more.set(false);
            self.apply_rows();
            return;
        };

        let workspace_key = workspace.key().to_string();
        let search_query = self.search_query.borrow().clone();
        let selected_tags = sorted_tags(&self.selected_tags.borrow());
        let loaded_limit = self.loaded_limit.get();
        let (sender, receiver) = mpsc::channel();

        thread::spawn({
            let workspace_key = workspace_key.clone();
            let search_query = search_query.clone();
            let selected_tags = selected_tags.clone();

            move || {
                let tags = load_tags.then(|| agent_history::workspace_tag_counts(&workspace_key));
                let rows = agent_history::list_sessions(
                    &workspace_key,
                    loaded_limit.saturating_add(1),
                    0,
                    Some(&search_query),
                    &selected_tags,
                );
                let _ = sender.send(AgentHistoryLoad {
                    workspace_key,
                    search_query,
                    selected_tags,
                    loaded_limit,
                    rows,
                    tags,
                });
            }
        });

        let agent_list = self.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || {
            match receiver.try_recv() {
                Ok(load) => {
                    agent_list.finish_history_load(load);
                    glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    agent_list.loading.set(false);
                    log::warn!("agent history load worker disconnected");
                    glib::ControlFlow::Break
                }
            }
        });
    }

    fn finish_history_load(&self, load: AgentHistoryLoad) {
        self.loading.set(false);
        if !self.history_load_matches(&load) {
            log::debug!(
                "discarding stale agent history load workspace={} query_len={} tags={} limit={}",
                load.workspace_key,
                load.search_query.len(),
                load.selected_tags.len(),
                load.loaded_limit
            );
            if self.workspace.borrow().is_some() {
                self.reload_workspace_history();
            }
            return;
        }

        if let Some(tags) = load.tags {
            match tags {
                Ok(tags) => self.apply_tags(tags),
                Err(err) => {
                    log::warn!("agent history tags load failed: {err}");
                    self.search_panel.set_tags(Vec::new());
                }
            }
        }

        match load.rows {
            Ok(mut rows) => {
                let has_more = rows.len() > load.loaded_limit;
                if has_more {
                    rows.truncate(load.loaded_limit);
                }
                self.has_more.set(has_more);
                self.history_rows.replace(rows);
                self.apply_rows();
            }
            Err(err) => {
                log::warn!("agent history load failed: {err}");
                self.has_more.set(false);
                self.history_rows.replace(Vec::new());
                self.apply_rows();
            }
        }
    }

    fn history_load_matches(&self, load: &AgentHistoryLoad) -> bool {
        self.workspace
            .borrow()
            .as_ref()
            .is_some_and(|workspace| workspace.key() == load.workspace_key)
            && *self.search_query.borrow() == load.search_query
            && sorted_tags(&self.selected_tags.borrow()) == load.selected_tags
            && self.loaded_limit.get() == load.loaded_limit
    }

    pub fn connect_new_chat<F>(&self, callback: F)
    where
        F: Fn(AgentLaunch) + 'static,
    {
        let callback = Rc::new(callback);

        self.app_button.connect_clicked({
            let callback = callback.clone();

            move |_| {
                callback(AgentLaunch::App);
            }
        });
        self.codex_cli_button.connect_clicked({
            let callback = callback.clone();

            move |_| {
                callback(AgentLaunch::CodexCli);
            }
        });
        self.agy_button.connect_clicked(move |_| {
            callback(AgentLaunch::Agy);
        });
    }

    pub fn add_session_row(
        &self,
        session_id: u64,
        provider: &'static dyn AgentProvider,
        title: &str,
        local_history_id: Option<i64>,
        state: AgentSessionState,
    ) {
        let last_seen_at_ms = self.active_session_last_seen_at_ms(local_history_id);
        let mut active_sessions = self.active_sessions.borrow_mut();
        if let Some(session) = active_sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
        {
            session.provider = provider;
            session.title = title.to_string();
            session.local_history_id = local_history_id;
            if local_history_id.is_some() {
                session.last_seen_at_ms = last_seen_at_ms;
            }
            session.state = state;
            drop(active_sessions);
            self.apply_rows();
            return;
        }

        if let Some(local_id) = local_history_id {
            log::debug!(
                "agent list restored row placed by history timestamp session_id={} local_id={} last_seen_at_ms={}",
                session_id,
                local_id,
                last_seen_at_ms
            );
        }

        active_sessions.insert(
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
        drop(active_sessions);
        self.apply_rows();
        self.select_session(session_id);
    }

    pub fn select_session(&self, session_id: u64) {
        if let Some(row) = row_for_identity(&self.list, RowIdentity::Active(session_id)) {
            self.select_row_without_callback(&row);
        }
    }

    pub fn connect_selected<F>(&self, callback: F)
    where
        F: Fn(AgentListSelection) + 'static,
    {
        self.selection_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_context_action<F>(&self, callback: F)
    where
        F: Fn(AgentListContextAction) + 'static,
    {
        self.context_action_callback
            .replace(Some(Rc::new(callback)));
    }

    pub fn connect_close_requested<F>(&self, callback: F)
    where
        F: Fn(u64) + 'static,
    {
        self.close_callback.replace(Some(Rc::new(callback)));
    }

    pub fn remove_session(&self, session_id: u64) -> bool {
        let before = self.active_sessions.borrow().len();
        self.active_sessions
            .borrow_mut()
            .retain(|session| session.session_id != session_id);
        self.apply_rows();
        before != self.active_sessions.borrow().len()
    }

    pub fn update_title(&self, session_id: u64, title: &str) {
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
            self.apply_rows();
        }
    }

    pub fn set_session_state(
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
            self.apply_rows();
        }
        changed
    }

    pub fn set_resource_usage(&self, session_id: u64, usage: Option<AgentResourceUsage>) {
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
            self.apply_rows();
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
        self.reload_workspace_history();
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
        self.reload_workspace_history();
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

        {
            let stats = self.history_monitor_stats.borrow();
            if stats.raw_events > 0 || stats.reloads > 0 {
                log::info!(
                    "agent history monitor stopped lifetime_ms={} raw_events={} suppressed_events={} reloads={}",
                    stats.started_at.elapsed().as_millis(),
                    stats.raw_events,
                    stats.suppressed_events,
                    stats.reloads
                );
            }
        }
        self.history_monitor_stats
            .replace(HistoryMonitorStats::default());

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
        self.history_db_signature
            .replace(history_db_signature(&craic_dir));

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
                    agent_list.history_monitor_stats.borrow_mut().raw_events += 1;
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
            if agent_list.loading.get() {
                agent_list.queue_history_reload();
                return glib::ControlFlow::Break;
            }
            if agent_list.history_db_changed() {
                let mut stats = agent_list.history_monitor_stats.borrow_mut();
                stats.reloads += 1;
                log::debug!(
                    "agent history monitor reloading raw_events={} suppressed_events={} reloads={}",
                    stats.raw_events,
                    stats.suppressed_events,
                    stats.reloads
                );
                drop(stats);
                agent_list.reload_workspace_history();
            } else {
                let mut stats = agent_list.history_monitor_stats.borrow_mut();
                stats.suppressed_events += 1;
                if stats.suppressed_events == 1 {
                    log::debug!("agent history monitor suppressed sidecar-only database events");
                }
            }
            glib::ControlFlow::Break
        });
        self.debounce_source.replace(Some(source_id));
    }

    fn acknowledge_history_db_signature(&self) {
        let Some(craic_dir) = crate::config::craic_dir() else {
            return;
        };
        self.history_db_signature
            .replace(history_db_signature(&craic_dir));
    }

    fn history_db_changed(&self) -> bool {
        let Some(craic_dir) = crate::config::craic_dir() else {
            return false;
        };
        let next = history_db_signature(&craic_dir);
        let mut current = self.history_db_signature.borrow_mut();
        if *current == next {
            return false;
        }
        *current = next;
        true
    }

    fn apply_tags(&self, tags: Vec<WorkspaceTag>) {
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

    fn apply_rows(&self) {
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
            elements.push((
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
                elements.push((
                    AgentRowKey::Header(group.clone()),
                    AgentRowRenderState::Header { label: group },
                ));
            }
            match row {
                TimelineRow::Active(session) => {
                    elements.push((
                        AgentRowKey::Active(session.session_id),
                        active_row_render_state(session, history_rows.as_slice()),
                    ));
                }
                TimelineRow::History(row) => {
                    elements.push((AgentRowKey::History(row.id), history_row_render_state(row)));
                }
            }
        }

        self.apply_row_commands(elements, close_cb);

        if let Some(selected) = selected.and_then(|identity| row_for_identity(&self.list, identity))
        {
            self.select_row_without_callback(&selected);
        }
    }

    fn apply_row_commands(
        &self,
        rows: Vec<(AgentRowKey, AgentRowRenderState)>,
        close_callback: Rc<dyn Fn(u64)>,
    ) {
        let desired = rows.iter().map(|(key, _)| key).collect::<HashSet<_>>();
        let removed = self
            .row_widgets
            .borrow()
            .keys()
            .filter(|key| !desired.contains(key))
            .cloned()
            .collect::<Vec<_>>();
        for key in removed {
            if let Some(row) = self.row_widgets.borrow_mut().remove(&key) {
                self.list.remove(&row);
            }
            self.row_states.borrow_mut().remove(&key);
        }

        for (index, (key, state)) in rows.into_iter().enumerate() {
            let existing = { self.row_widgets.borrow().get(&key).cloned() };
            let row = match existing {
                Some(row) => {
                    if self.row_states.borrow().get(&key) != Some(&state) {
                        update_agent_row(row.upcast_ref(), &state, close_callback.clone());
                    }
                    row
                }
                None => agent_row(&state, close_callback.clone()),
            };
            self.row_states.borrow_mut().insert(key.clone(), state);
            self.row_widgets.borrow_mut().insert(key, row.clone());
            if row.index() != index as i32 {
                if row.parent().is_some() {
                    self.list.remove(&row);
                }
                self.list.insert(&row, index as i32);
            }
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
            RowIdentity::History(local_id) => self
                .history_rows
                .borrow()
                .iter()
                .find(|history| history.id == local_id)
                .map(|history| AgentListContextTarget {
                    session_id: None,
                    local_id: Some(local_id),
                    loaded: false,
                    has_summary: history.task_description.is_some(),
                    terminal_session: history.provider_id != "codex-app",
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
                    terminal_session: session.provider.provider_id() != "codex-app",
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
