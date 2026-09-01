impl AgentChat {
    pub fn new(ctx: PageContext) -> Self {
        let prompt_bar = PromptBar::new();
        let local_workspace_path = ctx.local_workspace_path();
        prompt_bar.set_local_repo_path(local_workspace_path.as_deref());
        let workspace = ctx.workspace_ref();
        let initial_workspace_path = PathBuf::from(&workspace.root.absolute);
        let initial_workspace_history =
            agent_history::workspace_for_system_path(ctx.workspace_key(), workspace.root.absolute);

        let notebook = gtk::Notebook::builder()
            .show_tabs(false)
            .show_border(false)
            .hexpand(true)
            .vexpand(true)
            .build();
        let search_panel = SearchPanel::new("Search Agent Terminal");
        search_panel.set_clear_on_close(false);
        search_panel.set_options_visible(true);
        search_panel.set_navigation_visible(true);
        let search_widget = search_panel.widget();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&prompt_bar.root);
        search_panel.set_key_capture_widget(&search_widget);
        search_panel.install_shortcuts(&search_widget);
        root.append(&search_widget);
        root.append(&notebook);
        let focus_handlers = Rc::new(RefCell::new(Vec::<Box<dyn Fn(bool)>>::new()));
        if let Some(application) = ctx.window().and_then(|window| window.application()) {
            focus_handlers.borrow_mut().push(Box::new(move |focused| {
                set_terminal_conflicting_accels_enabled(&application, !focused);
            }));
        }

        let chat = Self {
            root,
            ctx,
            prompt_bar,
            search_panel,
            search_options: TerminalSearchOptions::new(),
            notebook,
            sessions: Rc::new(RefCell::new(Vec::new())),
            app_sessions: Rc::new(RefCell::new(Vec::new())),
            next_session_id: Rc::new(Cell::new(1)),
            working_directory: Rc::new(RefCell::new(initial_workspace_path)),
            workspace_history: Rc::new(RefCell::new(initial_workspace_history)),
            new_session_callback: Rc::new(RefCell::new(None)),
            title_callback: Rc::new(RefCell::new(None)),
            state_callback: Rc::new(RefCell::new(None)),
            resource_usage_callback: Rc::new(RefCell::new(None)),
            close_callback: Rc::new(RefCell::new(None)),
            history_callback: Rc::new(RefCell::new(None)),
            focus_handlers,
            usage_tracker: Rc::new(RefCell::new(ProcessUsageTracker::new())),
        };

        chat.connect_controls();
        chat.connect_search_controls();
        chat.start_status_polling();

        chat
    }

    pub fn set_workspace_from_context(&self) -> usize {
        let workspace = self.ctx.workspace_ref();
        let next_workspace_key = self.ctx.workspace_key();
        let current_workspace_key = self.workspace_history.borrow().key().to_string();
        let closed_sessions = if current_workspace_key != next_workspace_key {
            self.close_sessions_for_workspace_change(&current_workspace_key, &next_workspace_key)
        } else {
            0
        };

        self.working_directory
            .replace(PathBuf::from(&workspace.root.absolute));
        self.workspace_history
            .replace(agent_history::workspace_for_system_path(
                next_workspace_key,
                workspace.root.absolute,
            ));
        let local_workspace_path = self.ctx.local_workspace_path();
        self.prompt_bar
            .set_local_repo_path(local_workspace_path.as_deref());
        closed_sessions
    }

    fn close_sessions_for_workspace_change(
        &self,
        current_workspace_key: &str,
        next_workspace_key: &str,
    ) -> usize {
        let session_ids = self
            .sessions
            .borrow()
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        let app_session_ids = self
            .app_sessions
            .borrow()
            .iter()
            .map(AppChatSession::id)
            .collect::<Vec<_>>();

        if session_ids.is_empty() && app_session_ids.is_empty() {
            return 0;
        }

        log::info!(
            "agent workspace changed; closing active sessions current_workspace={} next_workspace={} count={}",
            current_workspace_key,
            next_workspace_key,
            session_ids.len() + app_session_ids.len()
        );
        for session_id in &session_ids {
            close_session(
                *session_id,
                &self.sessions,
                &self.notebook,
                &self.close_callback,
                &self.history_callback,
            );
        }
        for session_id in &app_session_ids {
            close_app_session(
                *session_id,
                &self.app_sessions,
                &self.notebook,
                &self.close_callback,
                &self.history_callback,
            );
        }
        session_ids.len() + app_session_ids.len()
    }

    pub fn show(&self) {
        if self.sessions.borrow().is_empty() && self.app_sessions.borrow().is_empty() {
            self.start_app();
        } else {
            self.focus_active_session();
        }
    }

    pub fn start_app(&self) {
        if let Err(error) = self.start_app_session(None) {
            self.ctx.show_error("Start Codex App Failed", &error);
        }
    }

    fn start_app_session(&self, restored: Option<&AgentSessionRow>) -> Result<u64, String> {
        let session_id = restored
            .map(|row| history_session_id(row.id))
            .transpose()?
            .unwrap_or_else(|| self.reserve_session_id());
        self.reserve_session_id_at_least(session_id);
        let session = if let Some(row) = restored {
            let thread_id = row.cli_session_id.clone().ok_or_else(|| {
                format!(
                    "Stored Codex App session local_id={} has no thread id.",
                    row.id
                )
            })?;
            AppChatSession::resume(session_id, self.ctx.clone(), thread_id, row.id, &row.title)?
        } else {
            AppChatSession::new(session_id, self.ctx.clone())?
        };
        let page = session.root();
        page.set_widget_name(&session_id.to_string());
        let label = gtk::Label::builder()
            .label(session.title())
            .ellipsize(pango::EllipsizeMode::End)
            .width_chars(12)
            .max_width_chars(18)
            .xalign(0.0)
            .build();
        let icon = gtk::Image::from_icon_name(provider::app::PROVIDER.session_icon_name());
        icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);
        let waiting_icon = gtk::Image::from_icon_name(WAITING_AGENT_SESSION_ICON);
        waiting_icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(AGENT_ICON_PIXEL_SIZE, AGENT_ICON_PIXEL_SIZE);
        let icon_stack = gtk::Stack::new();
        icon_stack.add_named(&icon, Some("icon"));
        icon_stack.add_named(&waiting_icon, Some("waiting"));
        icon_stack.add_named(&spinner, Some("spinner"));
        icon_stack.set_visible_child_name("spinner");
        let close_button = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close session")
            .valign(gtk::Align::Center)
            .build();
        close_button.add_css_class("flat");
        close_button.add_css_class("circular");
        let tab_label = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(6)
            .margin_end(6)
            .build();
        tab_label.append(&icon_stack);
        tab_label.append(&label);
        tab_label.append(&close_button);

        let page_num = self.notebook.append_page(&page, Some(&tab_label));
        self.notebook.set_tab_reorderable(&page, true);
        self.notebook.set_current_page(Some(page_num));
        close_button.connect_clicked({
            let sessions = self.app_sessions.clone();
            let notebook = self.notebook.clone();
            let root = self.root.clone();
            let close_callback = self.close_callback.clone();
            let history_callback = self.history_callback.clone();
            move |_| {
                request_close_app_session(
                    session_id,
                    &root,
                    &sessions,
                    &notebook,
                    &close_callback,
                    &history_callback,
                )
            }
        });
        session.connect_title_changed({
            let label = label.clone();
            let title_callback = self.title_callback.clone();
            move |session_id, title| {
                label.set_text(&title);
                label.set_tooltip_text(Some(&title));
                if let Some(callback) = title_callback.borrow().clone() {
                    callback(session_id, title);
                }
            }
        });
        session.connect_state_changed({
            let icon_stack = icon_stack.clone();
            let state_callback = self.state_callback.clone();
            move |session_id, state| {
                let agent_state = app_agent_session_state(&state);
                icon_stack.set_visible_child_name(match agent_state {
                    AgentSessionState::Active(AgentActiveState::Loading) => "spinner",
                    AgentSessionState::Active(AgentActiveState::Asking) => "waiting",
                    _ => "icon",
                });
                if let Some(callback) = state_callback.borrow().clone() {
                    callback(session_id, &provider::app::PROVIDER, agent_state);
                }
            }
        });
        session.connect_history_changed({
            let history_callback = self.history_callback.clone();
            move |_| notify_history_changed(&history_callback)
        });
        session.connect_thread_changed({
            let sessions = Rc::downgrade(&self.app_sessions);
            let workspace_history = self.workspace_history.clone();
            let notebook = self.notebook.clone();
            let close_callback = self.close_callback.clone();
            let new_session_callback = self.new_session_callback.clone();
            let history_callback = self.history_callback.clone();
            move |session_id, thread_id, title| {
                let Some(sessions) = sessions.upgrade() else {
                    return;
                };
                let Some(session) = app_session_by_id(&sessions, session_id) else {
                    log::warn!(
                        "Codex App thread update ignored for missing session session_id={session_id} thread_id={thread_id}"
                    );
                    return;
                };
                if thread_id.is_empty() {
                    if let Some(callback) = new_session_callback.borrow().clone() {
                        callback(
                            session_id,
                            &provider::app::PROVIDER,
                            title,
                            None,
                            app_agent_session_state(&session.state()),
                        );
                    }
                    return;
                }
                let existing = sessions
                    .borrow()
                    .iter()
                    .find(|candidate| {
                        candidate.id() != session_id
                            && candidate.thread_id().as_deref() == Some(thread_id.as_str())
                    })
                    .cloned();
                if let Some(existing) = existing {
                    if let Some(page_num) = notebook.page_num(&existing.root()) {
                        notebook.set_current_page(Some(page_num));
                    }
                    existing.focus();
                    log::info!(
                        "Codex App duplicate thread focused existing_session_id={} duplicate_session_id={session_id} thread_id={thread_id}",
                        existing.id()
                    );
                    let sessions = sessions.clone();
                    let notebook = notebook.clone();
                    let close_callback = close_callback.clone();
                    let history_callback = history_callback.clone();
                    glib::idle_add_local_once(move || {
                        close_app_session(
                            session_id,
                            &sessions,
                            &notebook,
                            &close_callback,
                            &history_callback,
                        );
                    });
                    return;
                }
                let result = agent_history::upsert_restorable_session(
                    agent_history::AgentSessionUpsert {
                        provider_id: provider::app::PROVIDER.provider_id().to_owned(),
                        workspace: workspace_history.borrow().clone(),
                        title: title.clone(),
                        initial_restore_state: RestoreState::Restorable,
                        session_uuid: Some(thread_id.clone()),
                    },
                    &thread_id,
                );
                match result {
                    Ok(row) => {
                        if let Some(previous_local_id) = session.local_history_id()
                            && previous_local_id != row.id
                            && let Err(error) = agent_history::mark_ended(previous_local_id)
                        {
                            log::warn!(
                                "failed marking previous Codex App session ended session_id={session_id} local_id={previous_local_id}: {error}"
                            );
                        }
                        session.set_local_history_id(row.id);
                        if let Some(callback) = new_session_callback.borrow().clone() {
                            callback(
                                session_id,
                                &provider::app::PROVIDER,
                                title,
                                Some(row.id),
                                app_agent_session_state(&session.state()),
                            );
                        }
                        notify_history_changed(&history_callback);
                        log::info!(
                            "Codex App session stored session_id={session_id} local_id={} thread_id={thread_id}",
                            row.id
                        );
                    }
                    Err(error) => log::warn!(
                        "failed storing Codex App session session_id={session_id} thread_id={thread_id}: {error}"
                    ),
                }
            }
        });
        session.connect_close_requested({
            let sessions = Rc::downgrade(&self.app_sessions);
            let notebook = self.notebook.clone();
            let close_callback = self.close_callback.clone();
            let history_callback = self.history_callback.clone();
            move |session_id| {
                if let Some(sessions) = sessions.upgrade() {
                    close_app_session(
                        session_id,
                        &sessions,
                        &notebook,
                        &close_callback,
                        &history_callback,
                    );
                }
            }
        });
        self.app_sessions.borrow_mut().push(session.clone());
        session.show();

        if let Some(callback) = self.new_session_callback.borrow().clone() {
            callback(
                session_id,
                &provider::app::PROVIDER,
                session.title(),
                session.local_history_id(),
                if restored.is_some() {
                    AgentSessionState::Active(AgentActiveState::Loading)
                } else {
                    AgentSessionState::Active(AgentActiveState::NewChat)
                },
            );
        }
        notify_session_state_changed(
            &self.state_callback,
            session_id,
            &provider::app::PROVIDER,
            AgentSessionState::Active(AgentActiveState::Loading),
        );
        Ok(session_id)
    }

    pub fn start_chat(&self, provider: &'static dyn AgentProvider) {
        let session_id = self.reserve_session_id();
        let title = provider.default_title();
        let session_uuid = agent_history::new_session_uuid();
        self.start_chat_with_id(session_id, &session_uuid, provider, &title);
    }

    fn start_chat_with_id(
        &self,
        session_id: u64,
        session_uuid: &str,
        provider: &'static dyn AgentProvider,
        title: &str,
    ) {
        if session_id >= self.next_session_id.get() {
            self.next_session_id.set(session_id + 1);
        }
        let system = self.ctx.system_ref();
        let workspace = self.ctx.workspace_ref();
        let shell = self.ctx.shell();
        let command = match provider.command(shell.as_deref(), &system, &workspace) {
            Ok(command) => command,
            Err(err) => {
                self.ctx.show_error("Start Agent Failed", &err);
                return;
            }
        };
        provider.shell_integration().log_session_create(
            session_id,
            provider,
            title,
            command.target_working_dir(),
            &command.display(),
        );
        let _ = self.create_session(
            session_id,
            session_uuid,
            provider,
            title,
            &command,
            None,
            AgentActiveState::NewChat,
        );
    }

    pub fn restore_session(&self, row: &AgentSessionRow) -> Result<u64, String> {
        if !row.restore_state.is_restorable() {
            log::info!(
                "agent history restore ignored local_id={} provider={} restore_state={}",
                row.id,
                row.provider_id,
                row.restore_state.as_str()
            );
            return Err("Agent session is not restorable.".to_string());
        }
        if row.provider_id == provider::app::PROVIDER.provider_id() {
            if let Some(session) = app_session_by_local_history_id(&self.app_sessions, row.id) {
                self.show_session(session.id());
                return Ok(session.id());
            }
            if let Some(thread_id) = row.cli_session_id.as_deref()
                && let Some(session) = app_session_by_thread_id(&self.app_sessions, thread_id)
            {
                self.show_session(session.id());
                return Ok(session.id());
            }
            let session_id = history_session_id(row.id)?;
            if session_by_id(&self.sessions, session_id).is_some()
                || app_session_by_id(&self.app_sessions, session_id).is_some()
            {
                return Err(format!(
                    "Craic session id {session_id} is already active for another agent session."
                ));
            }
            return self.start_app_session(Some(row));
        }
        let cli_session_id = row.cli_session_id.as_deref().ok_or_else(|| {
            format!(
                "Agent session local_id={} is marked restorable without a CLI session id.",
                row.id
            )
        })?;
        let provider = provider::all_providers()
            .iter()
            .copied()
            .find(|provider| provider.provider_id() == row.provider_id)
            .ok_or_else(|| format!("Unknown agent provider {}.", row.provider_id))?;
        let shell = self.ctx.shell();
        let command = provider.restore_command(
            shell.as_deref(),
            &self.ctx.system_ref(),
            &self.ctx.workspace_ref(),
            cli_session_id,
        )?;
        let session_id = history_session_id(row.id)?;
        if let Some(active_session) = session_by_id(&self.sessions, session_id) {
            if active_session.local_history_id.get() == Some(row.id) {
                self.show_session(session_id);
                log::info!(
                    "agent history restore focused already-active session local_id={} session_id={}",
                    row.id,
                    session_id
                );
                return Ok(session_id);
            }

            log::warn!(
                "agent history restore blocked by active session id collision local_id={} session_id={}",
                row.id,
                session_id
            );
            return Err(format!(
                "Craic session id {session_id} is already active for another agent session."
            ));
        }
        self.reserve_session_id_at_least(session_id);
        provider.shell_integration().log_session_create(
            session_id,
            provider,
            &row.title,
            &row.repo_path.display().to_string(),
            &command.display(),
        );
        let _ = self.create_session(
            session_id,
            &row.session_uuid,
            provider,
            &row.title,
            &command,
            Some(row.id),
            AgentActiveState::Loading,
        )?;
        notify_history_changed(&self.history_callback);
        log::info!(
            "agent history restore started local_id={} session_id={} provider={} cli_session_id={}",
            row.id,
            session_id,
            provider.provider_id(),
            cli_session_id
        );
        Ok(session_id)
    }

    pub fn show_session(&self, session_id: u64) -> bool {
        if let Some(session) = app_session_by_id(&self.app_sessions, session_id) {
            let root = session.root();
            if let Some(page_num) = self.notebook.page_num(&root) {
                self.notebook.set_current_page(Some(page_num));
            }
            session.focus();
            return true;
        }
        if let Some(session) = session_by_id(&self.sessions, session_id) {
            if let Some(page_num) = self.notebook.page_num(&session.root) {
                self.notebook.set_current_page(Some(page_num));
            }
            session.terminal.grab_focus();
            true
        } else {
            false
        }
    }

    pub fn add_file_reference(&self, file_path: &str) {
        self.show();
        let current_page = self
            .notebook
            .current_page()
            .and_then(|page_num| self.notebook.nth_page(Some(page_num)));
        if let Some(session) = current_page
            .as_ref()
            .and_then(|page| app_session_by_page(&self.app_sessions, page))
        {
            session.add_mention(file_path.to_owned());
            session.focus();
            return;
        }
        let Some(session) = current_page
            .as_ref()
            .and_then(|page| session_by_page(&self.sessions, page))
        else {
            return;
        };

        session
            .terminal
            .feed_child(format!("@{file_path} ").as_bytes());
        session.terminal.grab_focus();
    }

    pub fn connect_prompt_bar(self: &Rc<Self>) {
        self.prompt_bar.connect_prompt_selected({
            let chat = self.clone();

            move |selection| {
                chat.send_prompt_selection(selection);
            }
        });
    }

    pub fn connect_title_changed<F>(&self, callback: F)
    where
        F: Fn(u64, String) + 'static,
    {
        self.title_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_state_changed<F>(&self, callback: F)
    where
        F: Fn(u64, &'static dyn AgentProvider, AgentSessionState) + 'static,
    {
        self.state_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_resource_usage_changed<F>(&self, callback: F)
    where
        F: Fn(u64, Option<AgentResourceUsage>) + 'static,
    {
        self.resource_usage_callback
            .replace(Some(Rc::new(callback)));
    }

    pub fn connect_close_requested<F>(&self, callback: F)
    where
        F: Fn(u64) + 'static,
    {
        self.close_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_history_changed<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.history_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_new_session<F>(&self, callback: F)
    where
        F: Fn(u64, &'static dyn AgentProvider, String, Option<i64>, AgentSessionState) + 'static,
    {
        self.new_session_callback.replace(Some(Rc::new(callback)));
    }

    pub fn running_session_count(&self) -> usize {
        let terminal_count = self
            .sessions
            .borrow()
            .iter()
            .filter(|session| match session.state.get() {
                TerminalSessionState::Starting => {
                    let state = session.active_state.get();
                    let counts = active_state_counts_as_running(state);
                    if is_selected_session(session, &self.notebook) {
                        log::debug!(
                            "agent running count starting session_id={} provider={} active_state={:?} counts={}",
                            session.id,
                            session.provider.provider_id(),
                            state,
                            counts
                        );
                    }
                    counts
                }
                TerminalSessionState::Running => {
                    let log_scan = is_selected_session(session, &self.notebook);
                    let state = agent_shell_integration::active_state(
                        session.id,
                        session.provider,
                        &session.terminal,
                        log_scan,
                    );
                    session.active_state.set(state);
                    active_state_counts_as_running(state)
                }
                TerminalSessionState::Exited | TerminalSessionState::Closing => false,
            })
            .count();
        terminal_count
            + self
                .app_sessions
                .borrow()
                .iter()
                .filter(|session| session.running())
                .count()
    }

    pub fn request_close_session(&self, session_id: u64) {
        if app_session_by_id(&self.app_sessions, session_id).is_some() {
            request_close_app_session(
                session_id,
                &self.root,
                &self.app_sessions,
                &self.notebook,
                &self.close_callback,
                &self.history_callback,
            );
            return;
        }
        request_close_session(
            session_id,
            &self.root,
            &self.sessions,
            &self.notebook,
            &self.close_callback,
            &self.history_callback,
        );
    }

    pub fn request_unload_history_session(&self, local_id: i64) {
        if let Some(session) = app_session_by_local_history_id(&self.app_sessions, local_id) {
            request_close_app_session(
                session.id(),
                &self.root,
                &self.app_sessions,
                &self.notebook,
                &self.close_callback,
                &self.history_callback,
            );
            return;
        }
        request_unload_history_session(
            local_id,
            &self.root,
            &self.sessions,
            &self.notebook,
            &self.close_callback,
            &self.history_callback,
        );
    }

    pub fn close_history_session(&self, local_id: i64) {
        if let Some(session) = app_session_by_local_history_id(&self.app_sessions, local_id) {
            close_app_session(
                session.id(),
                &self.app_sessions,
                &self.notebook,
                &self.close_callback,
                &self.history_callback,
            );
            return;
        }
        let Some(session) = session_by_local_history_id(&self.sessions, local_id) else {
            return;
        };
        close_session(
            session.id,
            &self.sessions,
            &self.notebook,
            &self.close_callback,
            &self.history_callback,
        );
    }

    pub fn history_session_is_loaded(&self, local_id: i64) -> bool {
        session_by_local_history_id(&self.sessions, local_id).is_some()
            || app_session_by_local_history_id(&self.app_sessions, local_id).is_some()
    }

    pub fn loaded_history_session_status(
        &self,
        local_id: i64,
    ) -> Option<LoadedHistorySessionStatus> {
        if let Some(session) = app_session_by_local_history_id(&self.app_sessions, local_id) {
            let state = session.state();
            return Some(LoadedHistorySessionStatus {
                session_id: session.id(),
                terminal_state: app_chat_state_label(&state),
                active_state: match app_agent_session_state(&state) {
                    AgentSessionState::Active(state) => Some(state),
                    AgentSessionState::Inactive(_) => None,
                },
            });
        }
        let session = session_by_local_history_id(&self.sessions, local_id)?;
        Some(loaded_history_session_status(&session))
    }

    pub fn active_session_status(&self, session_id: u64) -> Option<ActiveSessionStatus> {
        if let Some(session) = app_session_by_id(&self.app_sessions, session_id) {
            return Some(ActiveSessionStatus {
                session_id,
                session_uuid: session.thread_id().unwrap_or_default(),
                local_history_id: session.local_history_id(),
                provider_id: provider::app::PROVIDER.provider_id(),
                title: session.title(),
                terminal_state: app_chat_state_label(&session.state()),
                active_state: match app_agent_session_state(&session.state()) {
                    AgentSessionState::Active(state) => Some(state),
                    AgentSessionState::Inactive(_) => None,
                },
            });
        }
        let session = session_by_id(&self.sessions, session_id)?;
        Some(active_session_status(&session))
    }

    pub fn set_active_session_cli_id(
        &self,
        session_id: u64,
        cli_session_id: &str,
    ) -> Result<i64, String> {
        if app_session_by_id(&self.app_sessions, session_id).is_some() {
            return Err(
                "App sessions use Codex thread IDs and do not accept a CLI session ID.".to_owned(),
            );
        }
        let session = session_by_id(&self.sessions, session_id)
            .ok_or_else(|| format!("Agent session {session_id} is not active."))?;
        let local_id = ensure_agent_history_session(
            &session,
            &self.workspace_history,
            &self.history_callback,
        )?;
        agent_history::set_manual_session_id(local_id, cli_session_id)?;
        notify_history_changed(&self.history_callback);
        Ok(local_id)
    }

    pub fn generate_active_session_summary(&self, session_id: u64) -> Result<(), String> {
        if app_session_by_id(&self.app_sessions, session_id).is_some() {
            return Err("App sessions use Codex's native thread preview and metadata.".to_owned());
        }
        let session = session_by_id(&self.sessions, session_id)
            .ok_or_else(|| format!("Agent session {session_id} is not active."))?;
        start_smart_summary(
            &session,
            &self.workspace_history,
            &self.history_callback,
            SmartSummaryMode::Manual,
        )
    }

    pub fn generate_history_session_summary(&self, local_id: i64) -> Result<(), String> {
        let session = session_by_local_history_id(&self.sessions, local_id)
            .ok_or_else(|| "Load the session before generating a summary.".to_string())?;
        start_smart_summary(
            &session,
            &self.workspace_history,
            &self.history_callback,
            SmartSummaryMode::Manual,
        )
    }

    fn reserve_session_id(&self) -> u64 {
        self.sync_next_session_id_with_history();
        let mut session_id = self.next_session_id.get().max(1);
        while session_by_id(&self.sessions, session_id).is_some()
            || app_session_by_id(&self.app_sessions, session_id).is_some()
        {
            session_id = session_id.saturating_add(1);
        }
        self.next_session_id.set(session_id.saturating_add(1));
        session_id
    }

    fn reserve_session_id_at_least(&self, session_id: u64) {
        if session_id >= self.next_session_id.get() {
            self.next_session_id.set(session_id.saturating_add(1));
        }
    }

    fn sync_next_session_id_with_history(&self) {
        match agent_history::max_local_session_id() {
            Ok(Some(max_id)) => match history_session_id(max_id) {
                Ok(max_session_id) => self.reserve_session_id_at_least(max_session_id),
                Err(err) => {
                    log::warn!("agent history max session id ignored: {err}");
                }
            },
            Ok(None) => {}
            Err(err) => {
                log::warn!("agent history max session id lookup failed: {err}");
            }
        }
    }

    fn focus_active_session(&self) {
        let current_page = self
            .notebook
            .current_page()
            .and_then(|page_num| self.notebook.nth_page(Some(page_num)));
        if let Some(session) = current_page
            .as_ref()
            .and_then(|page| app_session_by_page(&self.app_sessions, page))
        {
            session.focus();
            return;
        }
        if let Some(session) = current_page
            .as_ref()
            .and_then(|page| session_by_page(&self.sessions, page))
        {
            session.terminal.grab_focus();
        }
    }

    fn send_prompt_selection(&self, selection: Result<PromptSelection, String>) {
        match selection {
            Ok(selection) => self.send_prompt_to_active_terminal(&selection.content),
            Err(err) => self.ctx.show_error("Prompt Failed", &err),
        }
    }

    fn send_prompt_to_active_terminal(&self, content: &str) {
        self.show();
        let current_page = self
            .notebook
            .current_page()
            .and_then(|page_num| self.notebook.nth_page(Some(page_num)));
        if let Some(session) = current_page
            .as_ref()
            .and_then(|page| app_session_by_page(&self.app_sessions, page))
        {
            session.add_prompt(content);
            return;
        }
        let Some(session) = current_page
            .as_ref()
            .and_then(|page| session_by_page(&self.sessions, page))
        else {
            return;
        };

        session.terminal.paste_text(content);
        session.terminal.grab_focus();
    }

    fn connect_controls(&self) {
        self.notebook.connect_switch_page({
            let sessions = self.sessions.clone();
            let app_sessions = self.app_sessions.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move |_, page, _| {
                if let Some(session) = app_session_by_page(&app_sessions, page) {
                    session.focus();
                    return;
                }
                if let Some(session) = session_by_page(&sessions, page) {
                    apply_terminal_search(
                        &session.terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Keep,
                    );
                    session.terminal.grab_focus();
                }
            }
        });
    }

    fn connect_search_controls(&self) {
        self.search_panel.connect_query_changed({
            let sessions = self.sessions.clone();
            let notebook = self.notebook.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move |_| {
                if let Some(terminal) = active_terminal(&sessions, &notebook) {
                    apply_terminal_search(
                        &terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Next,
                    );
                }
            }
        });
        self.search_panel.connect_opened({
            let sessions = self.sessions.clone();
            let notebook = self.notebook.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move || {
                if let Some(terminal) = active_terminal(&sessions, &notebook) {
                    apply_terminal_search(
                        &terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Next,
                    );
                }
            }
        });
        self.search_panel.connect_closed({
            let sessions = self.sessions.clone();
            let notebook = self.notebook.clone();
            let search_panel = self.search_panel.clone();

            move || {
                if let Some(terminal) = active_terminal(&sessions, &notebook) {
                    let _ = terminal.search(None, false);
                    search_panel.set_status("");
                    log::debug!("agent terminal search cleared on close");
                }
            }
        });
        connect_terminal_search_option(
            &self.search_panel,
            SearchOption::CaseSensitive,
            self.search_options.case_sensitive.clone(),
            self.sessions.clone(),
            self.notebook.clone(),
            self.search_options.clone(),
        );
        connect_terminal_search_option(
            &self.search_panel,
            SearchOption::WholeWord,
            self.search_options.whole_word.clone(),
            self.sessions.clone(),
            self.notebook.clone(),
            self.search_options.clone(),
        );
        connect_terminal_search_option(
            &self.search_panel,
            SearchOption::Regex,
            self.search_options.regex.clone(),
            self.sessions.clone(),
            self.notebook.clone(),
            self.search_options.clone(),
        );
        self.search_panel.connect_previous({
            let sessions = self.sessions.clone();
            let notebook = self.notebook.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move || {
                if let Some(terminal) = active_terminal(&sessions, &notebook) {
                    apply_terminal_search(
                        &terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Previous,
                    );
                }
            }
        });
        self.search_panel.connect_next({
            let sessions = self.sessions.clone();
            let notebook = self.notebook.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move || {
                if let Some(terminal) = active_terminal(&sessions, &notebook) {
                    apply_terminal_search(
                        &terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Next,
                    );
                }
            }
        });
    }

    fn create_session(
        &self,
        session_id: u64,
        session_uuid: &str,
        provider: &'static dyn AgentProvider,
        title: &str,
        command: &CommandSpec,
        local_history_id: Option<i64>,
        initial_active_state: AgentActiveState,
    ) -> Result<AgentSession, String> {
        let terminal = configured_terminal(
            config::load().font_sizes.shell,
            &self.sessions,
            &self.search_panel,
        );
        let remote_image_uploads = if provider.provider_id() == "codex"
            && self.ctx.system_ref().provider_kind != ProviderKind::Local
            && let Some(shell) = self.ctx.shell()
        {
            let ctx = self.ctx.clone();
            let working_dir = self.ctx.workspace_ref().root;
            let uploads = Rc::new(RemoteImageUploads {
                shell: shell.clone(),
                working_dir: working_dir.clone(),
                images: RefCell::new(Vec::new()),
            });
            let handler_uploads = uploads.clone();
            terminal.set_file_drop_handler(move |terminal, paths| {
                if paths
                    .iter()
                    .any(|path| !super::remote_image::supported_image_path(path))
                {
                    ctx.show_error(
                        "Remote Image Upload Failed",
                        "Remote Codex CLI drops currently accept PNG, JPEG, GIF, WebP, and BMP images.",
                    );
                    return;
                }
                let completion = command_mailbox::once({
                    let ctx = ctx.clone();
                    let uploads = handler_uploads.clone();
                    move |result: Result<Vec<super::remote_image::RemoteImage>, String>| match result {
                        Ok(images) => {
                            uploads.images.borrow_mut().extend(images.iter().cloned());
                            let paths = images
                                .into_iter()
                                .map(|image| PathBuf::from(image.path))
                                .collect::<Vec<_>>();
                            terminal.paste_file_paths(&paths);
                        }
                        Err(error) => ctx.show_error("Remote Image Upload Failed", &error),
                    }
                });
                super::remote_image::upload_images(
                    shell.clone(),
                    working_dir.clone(),
                    paths,
                    move |result| completion.send(result),
                );
            });
            Some(uploads)
        } else {
            None
        };
        install_focus_tracking(&terminal, &self.focus_handlers);
        terminal.connect_activation({
            let ctx = self.ctx.clone();
            move |activation| handle_agent_terminal_activation(&ctx, activation)
        });
        let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        root.set_child(Some(&terminal.widget()));

        let session_name = session_id.to_string();
        root.set_widget_name(&session_name);

        let display_title = if initial_active_state == AgentActiveState::NewChat {
            provider.default_title()
        } else {
            title.to_string()
        };
        let label = gtk::Label::builder()
            .label(&display_title)
            .ellipsize(pango::EllipsizeMode::End)
            .width_chars(12)
            .max_width_chars(18)
            .xalign(0.0)
            .build();

        let close_button = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close session")
            .valign(gtk::Align::Center)
            .build();
        close_button.add_css_class("flat");
        close_button.add_css_class("circular");

        let icon = gtk::Image::from_icon_name(provider.session_icon_name());
        let waiting_icon = gtk::Image::from_icon_name(WAITING_AGENT_SESSION_ICON);
        icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);
        waiting_icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(AGENT_ICON_PIXEL_SIZE, AGENT_ICON_PIXEL_SIZE);
        spinner.set_valign(gtk::Align::Center);

        let icon_stack = gtk::Stack::builder().build();
        icon_stack.add_named(&icon, Some("icon"));
        icon_stack.add_named(&waiting_icon, Some("waiting"));
        icon_stack.add_named(&spinner, Some("spinner"));
        icon_stack.set_visible_child_name("icon");

        let tab_label = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(6)
            .margin_end(6)
            .build();
        tab_label.append(&icon_stack);
        tab_label.append(&label);
        tab_label.append(&close_button);

        let page_num = self.notebook.append_page(&root, Some(&tab_label));
        self.notebook.set_tab_reorderable(&root, true);
        self.notebook.set_current_page(Some(page_num));

        close_button.connect_clicked({
            let sessions = self.sessions.clone();
            let notebook = self.notebook.clone();
            let root = self.root.clone();
            let close_callback = self.close_callback.clone();
            let history_callback = self.history_callback.clone();

            move |_| {
                request_close_session(
                    session_id,
                    &root,
                    &sessions,
                    &notebook,
                    &close_callback,
                    &history_callback,
                );
            }
        });

        let child_pid = Rc::new(Cell::new(None));
        let state = Rc::new(Cell::new(TerminalSessionState::Starting));
        let active_state = Rc::new(Cell::new(initial_active_state));
        let loading_poll_count = Rc::new(Cell::new(0));
        let summary_requested = Rc::new(Cell::new(false));
        let summary_in_flight = Rc::new(Cell::new(false));

        install_exit_key_handler(
            session_id,
            &terminal,
            &state,
            &self.sessions,
            &self.notebook,
            &self.close_callback,
            &self.history_callback,
        );

        connect_child_exit(
            session_id,
            provider,
            &terminal,
            &label,
            &display_title,
            &child_pid,
            &state,
            provider.shell_integration(),
            &self.state_callback,
        );
        let title_locked = Rc::new(Cell::new(!provider::is_default_agent_title(title)));
        let local_history_id = Rc::new(Cell::new(local_history_id));
        connect_title_updates(
            session_id,
            session_uuid,
            provider,
            &terminal,
            &label,
            &state,
            &title_locked,
            &active_state,
            &local_history_id,
            &self.notebook,
            &self.workspace_history,
            &self.state_callback,
            &self.title_callback,
            &self.history_callback,
        );
        if let Err(err) = spawn_command(
            &terminal,
            command,
            &child_pid,
            &state,
            provider.shell_integration(),
            session_id,
            provider,
            &self.state_callback,
        ) {
            self.notebook.remove_page(Some(page_num));
            return Err(err);
        }

        let session = AgentSession {
            id: session_id,
            session_uuid: session_uuid.to_string(),
            provider,
            root,
            terminal,
            child_pid,
            state,
            active_state,
            icon_stack,
            label,
            title_locked,
            local_history_id,
            loading_poll_count,
            summary_requested,
            summary_in_flight,
            _remote_image_uploads: remote_image_uploads,
        };

        self.sessions.borrow_mut().push(session.clone());
        session.terminal.grab_focus();

        if let Some(ref cb) = *self.new_session_callback.borrow() {
            cb(
                session_id,
                provider,
                display_title.clone(),
                session.local_history_id.get(),
                AgentSessionState::Active(initial_active_state),
            );
        }
        notify_session_state_changed(
            &self.state_callback,
            session_id,
            provider,
            AgentSessionState::Active(initial_active_state),
        );

        Ok(session)
    }

    fn start_status_polling(&self) {
        let sessions = self.sessions.clone();
        let state_callback = self.state_callback.clone();
        let resource_usage_callback = self.resource_usage_callback.clone();
        let usage_tracker = self.usage_tracker.clone();
        let title_callback = self.title_callback.clone();
        let history_callback = self.history_callback.clone();
        let workspace_history = self.workspace_history.clone();
        let notebook = self.notebook.clone();
        let ctx = self.ctx.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(1000), move || {
            let borrowed_sessions = sessions.borrow();
            let session_ids = borrowed_sessions
                .iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            let process_snapshot = borrowed_sessions
                .iter()
                .any(|session| session.state.get() == TerminalSessionState::Running)
                .then(ProcessSnapshot::read)
                .flatten();

            for session in borrowed_sessions.iter() {
                let terminal_state = session.state.get();
                let previous_session_state = match terminal_state {
                    TerminalSessionState::Starting => {
                        AgentSessionState::Active(session.active_state.get())
                    }
                    TerminalSessionState::Running => {
                        AgentSessionState::Active(session.active_state.get())
                    }
                    TerminalSessionState::Exited | TerminalSessionState::Closing => {
                        AgentSessionState::Inactive(AgentInactiveState::Dead)
                    }
                };
                let process_running = terminal_state == TerminalSessionState::Running;
                let session_state = session_state_for_poll(session, terminal_state);
                let active_state = match session_state {
                    AgentSessionState::Active(state) => Some(state),
                    AgentSessionState::Inactive(_) => None,
                };
                if let Some(active_state) = active_state {
                    session.active_state.set(active_state);
                }
                let is_loading = active_state == Some(AgentActiveState::Loading);
                let loading_poll_count = if is_loading {
                    let next = session.loading_poll_count.get().saturating_add(1);
                    session.loading_poll_count.set(next);
                    next
                } else {
                    session.loading_poll_count.replace(0)
                };
                match active_state {
                    Some(AgentActiveState::Loading) => {
                        session.icon_stack.set_visible_child_name("spinner");
                    }
                    Some(AgentActiveState::Asking) => {
                        session.icon_stack.set_visible_child_name("waiting");
                    }
                    Some(AgentActiveState::NewChat | AgentActiveState::Idle) | None => {
                        session.icon_stack.set_visible_child_name("icon");
                    }
                }
                if process_running
                    && loading_poll_count > 1
                    && matches!(
                        active_state,
                        Some(AgentActiveState::Idle | AgentActiveState::Asking)
                    )
                {
                    notify_agent_turn_complete(
                        &ctx,
                        session.id,
                        session.provider,
                        active_state.expect("active state checked above"),
                        session.label.text().as_str(),
                    );
                }
                if session_state != previous_session_state {
                    retry_empty_codex_mapping_on_status_change(
                        session,
                        previous_session_state,
                        session_state,
                        &history_callback,
                    );
                    if let Some(ref cb) = *state_callback.borrow() {
                        cb(session.id, session.provider, session_state);
                    }
                }
                update_resource_usage(
                    session,
                    process_running,
                    process_snapshot.as_ref(),
                    &usage_tracker,
                    &resource_usage_callback,
                );
                maybe_start_smart_summary(session, &workspace_history, &history_callback);

                if session.title_locked.get() {
                    continue;
                }

                let log_scan = is_selected_session(session, &notebook);
                if let Some(title) = agent_shell_integration::session_title(
                    session.provider,
                    &session.terminal,
                    log_scan,
                ) {
                    if log_scan {
                        log::debug!(
                            "agent title parsed session_id={} provider={} title={}",
                            session.id,
                            session.provider.label(),
                            agent_shell_integration::log_preview(
                                &title,
                                agent_shell_integration::TERMINAL_LOG_PREVIEW_CHARS
                            )
                        );
                    }
                    if session.label.text().as_str() == title.as_str() {
                        session.title_locked.set(true);
                        continue;
                    }

                    session.label.set_label(&title);
                    session.title_locked.set(true);
                    if session.active_state.get() == AgentActiveState::NewChat {
                        let next_active_state = match terminal_state {
                            TerminalSessionState::Running => agent_shell_integration::active_state(
                                session.id,
                                session.provider,
                                &session.terminal,
                                log_scan,
                            ),
                            TerminalSessionState::Starting => AgentActiveState::NewChat,
                            TerminalSessionState::Exited | TerminalSessionState::Closing => {
                                AgentActiveState::Idle
                            }
                        };
                        if log_scan {
                            log::debug!(
                                "agent title update active state session_id={} provider={} terminal_state={:?} next_active_state={:?}",
                                session.id,
                                session.provider.provider_id(),
                                terminal_state,
                                next_active_state
                            );
                        }
                        session.active_state.set(next_active_state);
                        notify_session_state_changed(
                            &state_callback,
                            session.id,
                            session.provider,
                            AgentSessionState::Active(next_active_state),
                        );
                    }
                    persist_agent_session_title(
                        session.id,
                        session.provider,
                        &title,
                        &workspace_history,
                        &session.local_history_id,
                        &session.session_uuid,
                        &history_callback,
                    );
                    if let Some(ref cb) = *title_callback.borrow() {
                        cb(session.id, title);
                    }
                }
            }
            usage_tracker.borrow_mut().retain_sessions(&session_ids);

            glib::ControlFlow::Continue
        });
    }
}
