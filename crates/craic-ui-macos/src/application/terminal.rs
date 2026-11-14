impl AppDelegate {
    fn show_native_quick_command_sheet(&self) {
        let (Some(window), Some(handle)) = (
            self.ivars().window.get(),
            self.ivars().workspace_handle.borrow().clone(),
        ) else {
            self.present_path_action_error(
                "Run Failed",
                "Open a workspace before running a command.",
            );
            return;
        };
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 26.0)),
        );
        input.setPlaceholderString(Some(&NSString::from_str("Command")));
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Run in Integrated Terminal"));
        alert.setInformativeText(&NSString::from_str(
            "Run a shell command at the active workspace root.",
        ));
        alert.setAccessoryView(Some(&input));
        alert.addButtonWithTitle(&NSString::from_str("Run"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let command_text = completion_input.stringValue().to_string();
            let command_text = command_text.trim();
            if command_text.is_empty() {
                return;
            }
            let arguments = vec!["-c".to_string(), command_text.to_string()];
            let command = match handle.terminal_command("sh", &arguments) {
                Ok(command) => command,
                Err(error) => {
                    delegate.present_path_action_error("Run Failed", &error);
                    return;
                }
            };
            let title = command_text.chars().take(48).collect::<String>();
            let files = handle.workspace_files();
            let root = files.root();
            if let Err(error) = delegate.spawn_native_terminal_command_with_directory(
                command,
                title,
                files.copy_path(&root),
                files.local_path(&root),
            ) {
                delegate.present_path_action_error("Run Failed", &error);
                return;
            }
            delegate.set_native_terminal_visible(true);
            log::info!("native explicit terminal command started");
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        alert.window().makeFirstResponder(Some(&input));
    }

    fn spawn_native_terminal_session(&self) -> Result<(), String> {
        let handle = self
            .ivars()
            .workspace_handle
            .borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| "Open a workspace before starting a terminal.".to_string())?;
        let (command, title) = handle.interactive_shell_command()?;
        let files = handle.workspace_files();
        let root = files.root();
        self.spawn_native_terminal_command_with_directory(
            command,
            title,
            files.copy_path(&root),
            files.local_path(&root),
        )
    }

    fn activate_native_agent_page(&self, entering: bool) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if entering {
            agents.default_launch_attempted.set(false);
        }
        let existing = self.ivars().active_agent_terminal_id.get().or_else(|| {
            self.ivars()
                .terminal_sessions
                .borrow()
                .iter()
                .find(|session| session.placement == NativeTerminalPlacement::Agent)
                .map(|session| session.id)
        });
        if let Some(id) = existing {
            agents.default_launch_attempted.set(true);
            if entering {
                self.activate_native_terminal_session(id);
                self.set_native_agent_terminal_visible(true);
            }
        } else if self.ivars().workspace_handle.borrow().is_some()
            && self.ivars().agent_commands.get().is_some()
            && !agents.default_launch_attempted.replace(true)
        {
            let provider = NATIVE_AGENT_PROVIDERS
                .iter()
                .copied()
                .find(|provider| provider.provider_id() == "codex")
                .expect("shared agent registry includes Codex");
            self.start_native_terminal_agent(provider.provider_id(), provider.label(), None);
        }
    }

    fn start_native_terminal_agent(
        &self,
        program: &str,
        title: &str,
        history: Option<&craic_agent::agent_history::AgentSessionRow>,
    ) {
        let (Some(identity), Some(commands), Some(cancellation)) = (
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
            self.workspace_cancellation_token(),
        ) else {
            return;
        };
        let workspace = self
            .ivars()
            .workspaces
            .borrow()
            .iter()
            .find(|entry| entry.selection_id() == identity.workspace_id)
            .map(|entry| entry.workspace.clone());
        let Some(workspace) = workspace else {
            return;
        };
        if let Some(existing) = history.and_then(|history| {
            self.ivars()
                .terminal_sessions
                .borrow()
                .iter()
                .find(|session| session.agent_history_id == Some(history.id))
                .map(|session| session.id)
        }) {
            self.activate_native_terminal_session(existing);
            self.set_native_agent_terminal_visible(true);
            return;
        }
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            return;
        };
        let files = handle.workspace_files();
        let root = files.root();
        let cancellation = cancellation.child_token();
        let session_id = match self.create_native_terminal_session_at(
            None,
            ShellCommandActivity::Command,
            title.to_owned(),
            files.copy_path(&root),
            files.local_path(&root),
            NativeTerminalPlacement::Agent,
            None,
            Some(cancellation.clone()),
        ) {
            Ok(id) => id,
            Err(error) => {
                self.present_path_action_error("Unable to Start Agent", &error);
                return;
            }
        };
        if let Some(history) = history
            && let Some(session) = self
                .ivars()
                .terminal_sessions
                .borrow_mut()
                .iter_mut()
                .find(|session| session.id == session_id)
        {
            session.agent_history_id = Some(history.id);
            session.agent_title_locked = true;
            session.agent_active_state = AgentActiveState::Loading;
            session.agent_last_seen_ms = history.last_seen_at_ms;
            session.title_label = history.title.clone();
            session.title.setTitle(&NSString::from_str(&history.title));
        }
        self.refresh_native_agent_thread_rows();
        let command = NativeAgentCommand::PrepareTerminal {
            identity,
            session_id,
            workspace,
            provider_id: program.to_owned(),
            history: history.cloned(),
            cancellation,
        };
        if let Err(error) = commands.try_send(command) {
            self.finish_native_terminal_close(session_id);
            self.present_path_action_error(
                "Unable to Start Agent",
                &format!("The agent launch could not be queued: {error}"),
            );
        } else {
            log::info!(
                "native agent terminal shown before preparation session={session_id} provider={program}"
            );
        }
    }

    fn finish_native_terminal_agent_start(
        &self,
        identity: &AgentIdentity,
        session_id: isize,
        program: &str,
        history: Option<craic_agent::agent_history::AgentSessionRow>,
        result: Result<craic_system::terminal::TerminalCommand, String>,
    ) {
        if !self.agent_event_is_current(identity) {
            return;
        }
        let Some(provider) = NATIVE_AGENT_PROVIDERS
            .iter()
            .copied()
            .find(|provider| provider.provider_id() == program)
        else {
            return;
        };
        let title = provider.label();
        let terminal = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == session_id)
            .filter(|session| {
                session
                    .launch_cancellation
                    .as_ref()
                    .is_some_and(|token| !token.is_cancelled())
            })
            .map(|session| session.view.clone());
        let Some(terminal) = terminal else {
            log::debug!("ignoring native agent preparation after close session={session_id}");
            return;
        };
        let command = match result {
            Ok(command) => command,
            Err(error) => {
                self.finish_native_terminal_close(session_id);
                self.present_path_action_error("Unable to Start Agent", &error);
                return;
            }
        };
        let remote_media = if program == "codex" {
            match self.native_terminal_remote_media_context() {
                Ok(context) => context,
                Err(error) => {
                    self.finish_native_terminal_close(session_id);
                    self.present_path_action_error("Unable to Start Agent", &error);
                    return;
                }
            }
        } else {
            None
        };
        if let Err(error) = terminal.spawn(command, title) {
            self.finish_native_terminal_close(session_id);
            self.present_path_action_error("Unable to Start Agent", &error);
            return;
        }
        if let Some(session) = self
            .ivars()
            .terminal_sessions
            .borrow_mut()
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.launch_cancellation.take();
            session.remote_media = remote_media;
            session.agent_active_state = if history.is_some() {
                AgentActiveState::Loading
            } else {
                AgentActiveState::NewChat
            };
        }
        self.refresh_native_agent_thread_rows();
        self.restore_agents_page_badge();
        self.request_native_cli_history(false);
        log::info!(
            "native terminal agent started provider={program} title={title} placement=agents-detail"
        );
    }

    fn native_terminal_remote_media_context(
        &self,
    ) -> Result<Option<NativeTerminalRemoteMedia>, String> {
        let workspace_id = self
            .ivars()
            .active_workspace_id
            .borrow()
            .clone()
            .ok_or_else(|| "Open a workspace before starting an agent session.".to_string())?;
        let workspace = self
            .ivars()
            .workspaces
            .borrow()
            .iter()
            .find(|entry| entry.selection_id() == workspace_id)
            .map(|entry| entry.workspace.clone())
            .ok_or_else(|| "The active workspace configuration is unavailable.".to_string())?;
        let craic_config::WorkspaceProvider::Ssh { host } = workspace.provider else {
            return Ok(None);
        };
        let provider = SshProvider::new(SshProviderConfig::new(host));
        let workspace_ref = provider.workspace_for_remote_path(workspace.path);
        let shell = provider
            .shell(&workspace_ref)
            .ok_or_else(|| "Shell access is unavailable for this SSH workspace.".to_string())?;
        let cancellation = self
            .workspace_cancellation_token()
            .ok_or_else(|| "Workspace cancellation is unavailable.".to_string())?
            .child_token();
        Ok(Some(NativeTerminalRemoteMedia {
            workspace_id,
            shell,
            working_dir: workspace_ref.root,
            cancellation,
        }))
    }

    fn spawn_native_terminal_command(
        &self,
        command: ShellCommandSpec,
        title: String,
    ) -> Result<(), String> {
        let working_directory = command.working_dir.absolute.clone();
        self.spawn_native_terminal_command_with_directory(command, title, working_directory, None)
    }

    fn spawn_native_terminal_command_with_directory(
        &self,
        command: ShellCommandSpec,
        title: String,
        working_directory_label: String,
        local_working_directory: Option<PathBuf>,
    ) -> Result<(), String> {
        self.spawn_native_terminal_command_with_directory_at(
            command,
            title,
            working_directory_label,
            local_working_directory,
            NativeTerminalPlacement::General,
            None,
        )
    }

    fn spawn_native_terminal_command_with_directory_at(
        &self,
        command: ShellCommandSpec,
        title: String,
        working_directory_label: String,
        local_working_directory: Option<PathBuf>,
        placement: NativeTerminalPlacement,
        remote_media: Option<NativeTerminalRemoteMedia>,
    ) -> Result<(), String> {
        self.spawn_native_terminal_request_at(
            command.terminal_command(),
            command.activity,
            title,
            working_directory_label,
            local_working_directory,
            placement,
            remote_media,
        )
    }

    fn spawn_native_terminal_request_at(
        &self,
        mut command: craic_system::terminal::TerminalCommand,
        mut activity: ShellCommandActivity,
        title: String,
        working_directory_label: String,
        local_working_directory: Option<PathBuf>,
        placement: NativeTerminalPlacement,
        remote_media: Option<NativeTerminalRemoteMedia>,
    ) -> Result<(), String> {
        if activity == ShellCommandActivity::LocalInteractiveShell {
            let mut wrapped_arguments = vec![
                "-c".to_string(),
                NATIVE_LOCAL_SHELL_ACTIVITY_WRAPPER.to_string(),
                "craic-shell-wrapper".to_string(),
                NATIVE_LOCAL_SHELL_ACTIVITY_MONITOR.to_string(),
                command
                    .program
                    .to_str()
                    .ok_or_else(|| "Terminal program is not valid UTF-8".to_owned())?
                    .to_owned(),
            ];
            wrapped_arguments.extend(
                command
                    .args
                    .iter()
                    .map(|argument| {
                        argument
                            .to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "Terminal argument is not valid UTF-8".to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
            command.program = "/bin/sh".into();
            command.args = wrapped_arguments.into_iter().map(Into::into).collect();
            activity = ShellCommandActivity::ReportedInteractiveShell;
        }
        self.create_native_terminal_session_at(
            Some(command),
            activity,
            title,
            working_directory_label,
            local_working_directory,
            placement,
            remote_media,
            None,
        )
        .map(|_| ())
    }

    fn create_native_terminal_session_at(
        &self,
        command: Option<craic_system::terminal::TerminalCommand>,
        activity: ShellCommandActivity,
        title: String,
        working_directory_label: String,
        local_working_directory: Option<PathBuf>,
        placement: NativeTerminalPlacement,
        remote_media: Option<NativeTerminalRemoteMedia>,
        launch_cancellation: Option<WorkspaceCancellationToken>,
    ) -> Result<isize, String> {
        let preparing = launch_cancellation.is_some();
        let (stack, strip) = match placement {
            NativeTerminalPlacement::General => (
                self.ivars().terminal_stack.get(),
                self.ivars().terminal_tab_strip.get(),
            ),
            NativeTerminalPlacement::Agent => {
                let agents = self.ivars().agents.get();
                (agents.map(|agents| &agents.terminal_stack), None)
            }
        };
        let stack = stack.ok_or_else(|| "The native terminal view is unavailable.".to_string())?;
        let id = self.ivars().next_terminal_id.get().max(1);
        self.ivars().next_terminal_id.set(id.saturating_add(1));
        let terminal = TerminalMetalView::new(
            stack.bounds(),
            self.ivars().font_sizes.get().shell,
            self.mtm(),
        );
        terminal.attach_activation_delegate(self, id);
        terminal.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        if let Some(command) = command {
            terminal.spawn(command, &title)?;
        } else {
            terminal.setAccessibilityLabel(Some(&NSString::from_str(&format!("Starting {title}"))));
        }
        let agent_provider_label =
            (placement == NativeTerminalPlacement::Agent).then(|| title.clone());
        let agent_provider = agent_provider_label.as_deref().and_then(|label| {
            NATIVE_AGENT_PROVIDERS
                .iter()
                .copied()
                .find(|provider| provider.label() == label)
        });
        let title = agent_provider.map_or(title, |provider| provider.default_title());

        let tab = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(190.0, 32.0)),
        );
        let title_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&title),
                Some(self),
                Some(sel!(selectTerminalSession:)),
                self.mtm(),
            )
        };
        title_button.setFrame(NSRect::new(
            NSPoint::new(0.0, 2.0),
            NSSize::new(158.0, 28.0),
        ));
        title_button.setButtonType(NSButtonType::Toggle);
        title_button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        title_button.setAlignment(NSTextAlignment::Left);
        title_button.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        title_button.setTag(id);
        title_button.setToolTip(Some(&NSString::from_str(&format!(
            "{title}\n{working_directory_label}"
        ))));
        let tab_menu = NSMenu::new(self.mtm());
        for (menu_title, action) in [
            (
                "Copy Working Directory",
                sel!(copyTerminalWorkingDirectory:),
            ),
            (
                "Reveal Working Directory in Finder",
                sel!(revealTerminalWorkingDirectory:),
            ),
        ] {
            let item = unsafe {
                tab_menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(menu_title),
                    Some(action),
                    &NSString::new(),
                )
            };
            item.setTag(id);
            unsafe { item.setTarget(Some(self)) };
        }
        tab_menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        for (menu_title, action) in [
            ("Move Left", sel!(moveTerminalSessionLeft:)),
            ("Move Right", sel!(moveTerminalSessionRight:)),
        ] {
            let item = unsafe {
                tab_menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(menu_title),
                    Some(action),
                    &NSString::new(),
                )
            };
            item.setTag(id);
            unsafe { item.setTarget(Some(self)) };
        }
        tab_menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        let close_menu_item = unsafe {
            tab_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Close Session"),
                Some(sel!(closeTerminalSessionFromMenu:)),
                &NSString::new(),
            )
        };
        close_menu_item.setTag(id);
        unsafe {
            close_menu_item.setTarget(Some(self));
            title_button.setMenu(Some(&tab_menu));
        }
        tab.addSubview(&title_button);

        let close_button = unsafe {
            NSButton::buttonWithImage_target_action(
                &NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("xmark"),
                    Some(&NSString::from_str("Close terminal session")),
                )
                .expect("macOS provides the close terminal SF Symbol"),
                Some(self),
                Some(sel!(closeTerminalSession:)),
                self.mtm(),
            )
        };
        close_button.setFrame(NSRect::new(
            NSPoint::new(160.0, 4.0),
            NSSize::new(26.0, 26.0),
        ));
        close_button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        close_button.setTag(id);
        close_button.setToolTip(Some(&NSString::from_str("Close terminal session")));
        tab.addSubview(&close_button);

        terminal.attach_title_button(&title_button);
        terminal.setHidden(true);
        if let Some(strip) = strip {
            strip.addSubview(&tab);
        }
        stack.addSubview(&terminal);
        self.ivars()
            .terminal_sessions
            .borrow_mut()
            .push(NativeTerminalSession {
                id,
                tab,
                title: title_button,
                title_label: title,
                agent_provider_label,
                agent_provider,
                agent_active_state: AgentActiveState::NewChat,
                agent_title_locked: false,
                agent_summary_requested: false,
                agent_summary_in_flight: false,
                agent_last_seen_ms: craic_agent::agent_history::unix_now_ms(),
                agent_history_id: None,
                launch_cancellation,
                view: terminal.clone(),
                working_directory: working_directory_label,
                local_working_directory,
                placement,
                activity,
                reported_task_active: false,
                auto_close_timer: None,
                remote_media,
            });
        self.layout_native_terminal_tabs();
        if placement == NativeTerminalPlacement::Agent {
            self.ensure_native_agent_terminal_usage_timer();
            self.refresh_native_agent_thread_rows();
            self.restore_agents_page_badge();
            if !preparing {
                self.request_native_cli_history(false);
            }
        }
        self.activate_native_terminal_session(id);
        match placement {
            NativeTerminalPlacement::General => self.set_native_terminal_visible(true),
            NativeTerminalPlacement::Agent => self.set_native_agent_terminal_visible(true),
        }
        terminal.focus_terminal();
        Ok(id)
    }

    fn layout_native_terminal_tabs(&self) {
        let sessions = self.ivars().terminal_sessions.borrow();
        let Some(strip) = self.ivars().terminal_tab_strip.get() else {
            return;
        };
        let matching = sessions
            .iter()
            .filter(|session| session.placement == NativeTerminalPlacement::General)
            .collect::<Vec<_>>();
        strip.setFrameSize(NSSize::new((matching.len() as f64 * 190.0).max(1.0), 32.0));
        for (index, session) in matching.into_iter().enumerate() {
            session.tab.setFrame(NSRect::new(
                NSPoint::new(index as f64 * 190.0, 0.0),
                NSSize::new(190.0, 32.0),
            ));
        }
    }

    fn ensure_native_agent_terminal_usage_timer(&self) {
        if self.ivars().agent_terminal_usage_timer.borrow().is_some() {
            return;
        }
        self.ivars()
            .agent_terminal_usage_tracker
            .borrow_mut()
            .get_or_insert_with(ProcessUsageTracker::new);
        // SAFETY: The repeating timer runs on AppKit's main run loop and targets a selector
        // implemented by this retained application delegate. It is invalidated when the last
        // terminal-agent session closes and during application shutdown.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                1.0,
                self.as_ref(),
                sel!(refreshAgentTerminalUsage:),
                None,
                true,
            )
        };
        self.ivars().agent_terminal_usage_timer.replace(Some(timer));
        self.sample_native_agent_terminal_usage();
        log::debug!("native terminal-agent resource polling started");
    }

    fn sample_native_agent_terminal_usage(&self) {
        self.refresh_native_terminal_agent_states();
        let sessions = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .filter(|session| session.placement == NativeTerminalPlacement::Agent)
            .filter_map(|session| {
                session
                    .view
                    .child_pid()
                    .and_then(|pid| i32::try_from(pid).ok())
                    .map(|pid| (session.id, pid))
            })
            .collect::<Vec<_>>();
        if sessions.is_empty() {
            self.stop_native_agent_terminal_usage_timer();
            return;
        }
        let Some(snapshot) = ProcessSnapshot::read() else {
            return;
        };
        let session_ids = sessions
            .iter()
            .filter_map(|(id, _)| u64::try_from(*id).ok())
            .collect::<Vec<_>>();
        let mut tracker_ref = self.ivars().agent_terminal_usage_tracker.borrow_mut();
        let tracker = tracker_ref.get_or_insert_with(ProcessUsageTracker::new);
        let mut usage = self.ivars().agent_terminal_usage.borrow_mut();
        for (id, pid) in sessions {
            let Some(session_id) = u64::try_from(id).ok() else {
                continue;
            };
            if let Some(sample) = tracker.sample(session_id, pid, &snapshot) {
                usage.insert(id, sample);
            } else {
                usage.remove(&id);
            }
        }
        usage.retain(|id, _| session_ids.contains(&u64::try_from(*id).unwrap_or_default()));
        tracker.retain_sessions(&session_ids);
        drop(usage);
        drop(tracker_ref);
        self.update_native_agent_terminal_usage_cards();
    }

    fn update_native_agent_terminal_usage_cards(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let usage = self.ivars().agent_terminal_usage.borrow();
        let sessions = self.ivars().terminal_sessions.borrow();
        let cards = agents.terminal_cards.borrow();
        for session in sessions
            .iter()
            .filter(|session| session.placement == NativeTerminalPlacement::Agent)
        {
            let Some(card) = cards.get(&session.id) else {
                continue;
            };
            let resource = if session.launch_cancellation.is_some() {
                "Starting terminal…".to_owned()
            } else if session.view.is_active() {
                usage
                    .get(&session.id)
                    .copied()
                    .map(AgentResourceUsage::sidebar_label)
                    .unwrap_or_else(|| "Measuring resources…".to_owned())
            } else {
                "Session ended".to_owned()
            };
            let provider = session
                .agent_provider_label
                .as_deref()
                .unwrap_or("Terminal agent");
            let state = if session.launch_cancellation.is_some() {
                "Starting"
            } else if session.view.is_active() {
                match session.agent_active_state {
                    AgentActiveState::NewChat => "New chat",
                    AgentActiveState::Idle => "Idle",
                    AgentActiveState::Loading => "Working",
                    AgentActiveState::Asking => "Waiting for input",
                }
            } else {
                "Exited"
            };
            card.resource.setStringValue(&NSString::from_str(&resource));
            card.metadata
                .setStringValue(&NSString::from_str(&format!("{provider} · {state}")));
            card.selector
                .setAccessibilityLabel(Some(&NSString::from_str(&format!(
                    "{}\n{provider} · {state}\n{resource}",
                    session.title_label
                ))));
        }
    }

    fn update_native_agent_terminal_card_selection(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let selected_id = (!agents.terminal_panel.isHidden())
            .then(|| self.ivars().active_agent_terminal_id.get())
            .flatten();
        for (id, card) in agents.terminal_cards.borrow().iter() {
            let selected = selected_id == Some(*id);
            let border_color = if selected {
                NSColor::controlAccentColor()
            } else {
                NSColor::separatorColor()
            };
            let fill_color = if selected {
                NSColor::selectedContentBackgroundColor()
            } else {
                NSColor::controlBackgroundColor()
            };
            let primary_color = if selected {
                NSColor::selectedControlTextColor()
            } else {
                NSColor::labelColor()
            };
            let secondary_color = if selected {
                primary_color.clone()
            } else {
                NSColor::secondaryLabelColor()
            };
            card.container.setBorderColor(&border_color);
            card.container.setFillColor(&fill_color);
            card.icon.setContentTintColor(Some(&secondary_color));
            card.title.setTextColor(Some(&primary_color));
            card.metadata.setTextColor(Some(&secondary_color));
            card.resource.setTextColor(Some(&secondary_color));
            card.selector.setState(if selected {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    fn stop_native_agent_terminal_usage_timer(&self) {
        if let Some(timer) = self.ivars().agent_terminal_usage_timer.borrow_mut().take() {
            timer.invalidate();
            log::debug!("native terminal-agent resource polling stopped");
        }
        self.ivars().agent_terminal_usage.borrow_mut().clear();
        self.ivars()
            .agent_terminal_usage_tracker
            .borrow_mut()
            .take();
    }

    fn activate_native_terminal_session(&self, id: isize) {
        let sessions = self.ivars().terminal_sessions.borrow();
        let Some(placement) = sessions
            .iter()
            .find(|session| session.id == id)
            .map(|session| session.placement)
        else {
            return;
        };
        let mut selected = None;
        for session in sessions
            .iter()
            .filter(|session| session.placement == placement)
        {
            let active = session.id == id;
            session.view.setHidden(!active);
            session.view.refresh_renderer_visibility();
            session.title.setState(if active {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            if active {
                selected = Some(session.view.clone());
            }
        }
        drop(sessions);
        if let Some(view) = selected {
            self.ivars().active_terminal_id.set(Some(id));
            match placement {
                NativeTerminalPlacement::General => {
                    self.ivars().active_general_terminal_id.set(Some(id));
                    self.set_native_terminal_visible(true);
                }
                NativeTerminalPlacement::Agent => {
                    self.ivars().active_agent_terminal_id.set(Some(id));
                    self.set_native_agent_terminal_visible(true);
                }
            }
            if self.ivars().terminal_search_visible.get() {
                self.show_native_terminal_search();
            } else {
                view.focus_terminal();
            }
            log::debug!("native terminal session selected id={id}");
        }
    }

    fn active_native_terminal_view(&self) -> Option<Retained<TerminalMetalView>> {
        let id = self.ivars().active_terminal_id.get()?;
        self.ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == id)
            .map(|session| session.view.clone())
    }

    fn native_terminal_placement_is_visible(&self, placement: NativeTerminalPlacement) -> bool {
        match placement {
            NativeTerminalPlacement::General => {
                self.ivars().terminal_visible.get()
                    && self
                        .ivars()
                        .terminal_panel
                        .get()
                        .is_some_and(|panel| !panel.isHidden())
            }
            NativeTerminalPlacement::Agent => {
                self.is_active_page("agents")
                    && self.ivars().agents.get().is_some_and(|agents| {
                        !agents.content_root.isHidden() && !agents.terminal_panel.isHidden()
                    })
            }
        }
    }

    pub(crate) fn adjust_native_terminal_font_size(&self, delta: f64) {
        let mut font_sizes = self.ivars().font_sizes.get();
        let next = craic_config::normalize_font_size(
            font_sizes.shell + delta,
            craic_config::DEFAULT_SHELL_FONT_SIZE,
        );
        if (next - font_sizes.shell).abs() < f64::EPSILON {
            return;
        }
        font_sizes.shell = next;
        craic_config::save_font_sizes(font_sizes);
        self.ivars().font_sizes.set(font_sizes);
        for session in self.ivars().terminal_sessions.borrow().iter() {
            session.view.set_font_size(next);
        }
        if let Some(settings) = self.ivars().commit_message_settings.get() {
            Self::populate_native_font_size_fields(settings, font_sizes);
            settings
                .font_status
                .setTextColor(Some(&NSColor::secondaryLabelColor()));
            settings
                .font_status
                .setStringValue(&NSString::from_str("Shell size updated."));
        }
        log::info!("native terminal font size adjusted size={next}");
    }

    fn adjust_native_active_font_size(&self, delta: f64) {
        if self
            .active_native_terminal_view()
            .is_some_and(|terminal| terminal.is_focused())
        {
            self.adjust_native_terminal_font_size(delta);
            return;
        }

        let mut font_sizes = self.ivars().font_sizes.get();
        let (next, surface) = match self.ivars().active_page_id.borrow().as_deref() {
            Some("changes" | "history") => {
                let next = craic_config::normalize_font_size(
                    font_sizes.diff + delta,
                    craic_config::DEFAULT_DIFF_FONT_SIZE,
                );
                if (next - font_sizes.diff).abs() < f64::EPSILON {
                    return;
                }
                font_sizes.diff = next;
                if let Some(diff) = self.ivars().diff_view.get() {
                    diff.set_font_size(next);
                }
                if let Some(history) = self.ivars().history.get() {
                    history.diff.set_font_size(next);
                }
                (next, "diff")
            }
            Some("files") => {
                let next = craic_config::normalize_font_size(
                    font_sizes.editor + delta,
                    craic_config::DEFAULT_EDITOR_FONT_SIZE,
                );
                if (next - font_sizes.editor).abs() < f64::EPSILON {
                    return;
                }
                font_sizes.editor = next;
                if let Some(files) = self.ivars().files.get() {
                    files
                        .preview_text
                        .setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(next, 0.0)));
                    files.preview_code.set_font_size(next);
                }
                (next, "editor")
            }
            Some("agents") => {
                self.adjust_native_terminal_font_size(delta);
                return;
            }
            _ => return,
        };
        craic_config::save_font_sizes(font_sizes);
        self.ivars().font_sizes.set(font_sizes);
        if let Some(settings) = self.ivars().commit_message_settings.get() {
            Self::populate_native_font_size_fields(settings, font_sizes);
            settings
                .font_status
                .setTextColor(Some(&NSColor::secondaryLabelColor()));
            settings
                .font_status
                .setStringValue(&NSString::from_str("Font size updated."));
        }
        log::info!("native active font size adjusted surface={surface} size={next}");
    }

    fn reset_native_active_font_size(&self) {
        if self
            .active_native_terminal_view()
            .is_some_and(|terminal| terminal.is_focused())
        {
            let current = self.ivars().font_sizes.get().shell;
            self.adjust_native_terminal_font_size(craic_config::DEFAULT_SHELL_FONT_SIZE - current);
            return;
        }
        let font_sizes = self.ivars().font_sizes.get();
        let delta = match self.ivars().active_page_id.borrow().as_deref() {
            Some("changes" | "history") => craic_config::DEFAULT_DIFF_FONT_SIZE - font_sizes.diff,
            Some("files") => craic_config::DEFAULT_EDITOR_FONT_SIZE - font_sizes.editor,
            Some("agents") => craic_config::DEFAULT_SHELL_FONT_SIZE - font_sizes.shell,
            _ => return,
        };
        self.adjust_native_active_font_size(delta);
    }

    pub(crate) fn native_terminal_files_dropped(
        &self,
        session_id: isize,
        paths: Vec<PathBuf>,
    ) -> bool {
        let context = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == session_id)
            .and_then(|session| session.remote_media.clone());
        let Some(context) = context else {
            return false;
        };
        if paths
            .iter()
            .any(|path| !remote_media::supported_path(path, RemoteMediaKind::Image))
        {
            self.present_path_action_error(
                "Remote Image Upload Failed",
                "Remote Codex CLI drops currently accept PNG, JPEG, GIF, WebP, and BMP images.",
            );
            return true;
        }
        let session_active = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == session_id)
            .is_some_and(|session| session.view.is_active());
        if !session_active || context.cancellation.is_cancelled() {
            log::debug!("remote Codex CLI image drop ignored for inactive session id={session_id}");
            return true;
        }
        let Some(commands) = self.ivars().terminal_media_commands.get() else {
            self.present_path_action_error(
                "Remote Image Upload Failed",
                "The remote image upload service is unavailable.",
            );
            return true;
        };
        let count = paths.len();
        if let Err(error) = commands.send(NativeTerminalMediaCommand::Upload {
            session_id,
            context,
            sources: paths,
        }) {
            self.present_path_action_error(
                "Remote Image Upload Failed",
                &format!("The remote image upload could not be queued: {error}"),
            );
        } else {
            log::info!(
                "remote Codex CLI image upload queued session={} count={}",
                session_id,
                count
            );
        }
        true
    }

    fn apply_terminal_remote_images(
        &self,
        workspace_id: &str,
        session_id: isize,
        result: Result<Vec<String>, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let terminal = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == session_id)
            .filter(|session| {
                session.view.is_active()
                    && session.remote_media.as_ref().is_some_and(|context| {
                        context.workspace_id == workspace_id && !context.cancellation.is_cancelled()
                    })
            })
            .map(|session| session.view.clone());
        let Some(terminal) = terminal else {
            return;
        };
        match result {
            Ok(paths) => {
                let paths = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
                terminal.paste_file_paths(&paths);
                log::info!(
                    "remote Codex CLI image paths pasted session={} count={}",
                    session_id,
                    paths.len()
                );
            }
            Err(error) => self.present_path_action_error("Remote Image Upload Failed", &error),
        }
    }

    pub(crate) fn native_terminal_session_exited(&self, id: isize, exit_code: Option<i32>) {
        self.native_provider_update_terminal_finished(id, exit_code == Some(0));
        let agent_session = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == id)
            .is_some_and(|session| session.placement == NativeTerminalPlacement::Agent);
        if let Some(session) = self
            .ivars()
            .terminal_sessions
            .borrow_mut()
            .iter_mut()
            .find(|session| session.id == id)
        {
            session.reported_task_active = false;
        }
        if exit_code == Some(0) && !agent_session {
            self.schedule_native_terminal_auto_close(id);
        } else if let Some(session) = self
            .ivars()
            .terminal_sessions
            .borrow_mut()
            .iter_mut()
            .find(|session| session.id == id)
            && let Some(timer) = session.auto_close_timer.take()
        {
            timer.invalidate();
        }
        if agent_session {
            self.refresh_native_agent_thread_rows();
            self.restore_agents_page_badge();
            self.request_native_cli_history(false);
        }
    }

    pub(crate) fn native_terminal_reported_activity_changed(&self, id: isize, active: bool) {
        let mut sessions = self.ivars().terminal_sessions.borrow_mut();
        let Some(session) = sessions.iter_mut().find(|session| session.id == id) else {
            return;
        };
        if session.activity != ShellCommandActivity::ReportedInteractiveShell
            || session.reported_task_active == active
        {
            return;
        }
        session.reported_task_active = active;
        let is_agent = session.placement == NativeTerminalPlacement::Agent;
        drop(sessions);
        log::debug!("native remote terminal activity changed id={id} active={active}");
        if is_agent {
            self.restore_agents_page_badge();
        }
    }

    pub(crate) fn native_terminal_session_interacted(&self, id: isize) {
        let Some((placement, exited_successfully)) = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == id)
            .map(|session| (session.placement, session.view.exited_successfully()))
        else {
            return;
        };
        let focus_moved = self.ivars().active_terminal_id.replace(Some(id)) != Some(id);
        if focus_moved {
            log::debug!("native terminal focus moved to session id={id}");
            if self.ivars().terminal_search_visible.get() {
                self.place_native_terminal_search(placement);
            }
        }
        if exited_successfully && placement == NativeTerminalPlacement::General {
            self.schedule_native_terminal_auto_close(id);
        }
    }

    pub(crate) fn native_terminal_session_title_changed(&self, id: isize, title: &str) {
        let is_agent = {
            let mut sessions = self.ivars().terminal_sessions.borrow_mut();
            let Some(session) = sessions.iter_mut().find(|session| session.id == id) else {
                return;
            };
            if session.placement == NativeTerminalPlacement::Agent {
                // The provider parser owns the conversation title; OSC titles also contain
                // transient spinner and permission states that must never rename a session.
                return;
            }
            if session.title_label == title {
                return;
            }
            session.title_label = title.to_owned();
            session.placement == NativeTerminalPlacement::Agent
        };
        if is_agent {
            if let Some(agents) = self.ivars().agents.get()
                && let Some(card) = agents.terminal_cards.borrow().get(&id)
            {
                card.title.setStringValue(&NSString::from_str(title));
            }
            self.update_native_agent_terminal_usage_cards();
        }
    }

    pub(crate) fn close_exited_native_terminal(&self, id: isize) {
        let exited = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == id)
            .is_some_and(|session| !session.view.is_active());
        if exited {
            self.finish_native_terminal_close(id);
        }
    }

    fn schedule_native_terminal_auto_close(&self, id: isize) {
        let information = NSString::from_str(&id.to_string());
        // SAFETY: The timer is scheduled on AppKit's main run loop, targets this live delegate,
        // uses a selector implemented above, and carries an NSString session identifier.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                TERMINAL_AUTO_CLOSE_IDLE_SECONDS,
                self.as_ref(),
                sel!(autoCloseTerminalSession:),
                Some(information.as_ref()),
                false,
            )
        };
        let mut sessions = self.ivars().terminal_sessions.borrow_mut();
        let Some(session) = sessions.iter_mut().find(|session| session.id == id) else {
            timer.invalidate();
            return;
        };
        if let Some(previous) = session.auto_close_timer.replace(timer) {
            previous.invalidate();
        }
        log::debug!(
            "scheduled native terminal auto-close id={id} seconds={}",
            TERMINAL_AUTO_CLOSE_IDLE_SECONDS
        );
    }

    fn show_native_terminal_search(&self) {
        let (Some(panel), Some(search), Some(window)) = (
            self.ivars().terminal_search_panel.get(),
            self.ivars().terminal_search.get(),
            self.ivars().window.get(),
        ) else {
            return;
        };
        let Some(placement) = self.ivars().active_terminal_id.get().and_then(|id| {
            self.ivars()
                .terminal_sessions
                .borrow()
                .iter()
                .find(|session| session.id == id)
                .map(|session| session.placement)
        }) else {
            return;
        };
        if !self.native_terminal_placement_is_visible(placement) {
            return;
        }
        self.ivars().terminal_search_visible.set(true);
        panel.setHidden(false);
        self.place_native_terminal_search(placement);
        window.makeFirstResponder(Some(search));
        // SAFETY: The retained search field is installed in the active AppKit window and this
        // action runs on the main thread.
        unsafe { search.selectText(None) };
        self.apply_native_terminal_search(TerminalSearchDirection::Next);
    }

    fn place_native_terminal_search(&self, placement: NativeTerminalPlacement) {
        let Some(panel) = self.ivars().terminal_search_panel.get() else {
            return;
        };
        let host = match placement {
            NativeTerminalPlacement::General => self.ivars().terminal_panel.get().cloned(),
            NativeTerminalPlacement::Agent => self
                .ivars()
                .agents
                .get()
                .map(|agents| agents.terminal_panel.clone()),
        };
        let Some(host) = host else {
            return;
        };
        if self.ivars().terminal_search_placement.get() != Some(placement) {
            panel.removeFromSuperview();
            host.addSubview(panel);
        }
        self.ivars().terminal_search_placement.set(Some(placement));
        // Both stacks must reclaim or reserve the search height after a reparent. Each layout
        // method only positions the shared panel when that surface currently owns it.
        self.layout_native_terminal_panel();
        self.layout_native_agent_terminal_panel();
    }

    fn hide_native_terminal_search(&self) {
        self.ivars().terminal_search_visible.set(false);
        if let Some(panel) = self.ivars().terminal_search_panel.get() {
            panel.setHidden(true);
        }
        if let Some(status) = self.ivars().terminal_search_status.get() {
            status.setStringValue(&NSString::new());
        }
        let active = self.ivars().active_terminal_id.get().and_then(|id| {
            self.ivars()
                .terminal_sessions
                .borrow()
                .iter()
                .find(|session| session.id == id)
                .map(|session| (session.placement, session.view.clone()))
        });
        if let Some((placement, terminal)) = active {
            terminal.clear_search();
            if self.native_terminal_placement_is_visible(placement) {
                terminal.focus_terminal();
            }
        }
        self.layout_native_terminal_panel();
        self.layout_native_agent_terminal_panel();
    }

    fn apply_native_terminal_search(&self, direction: TerminalSearchDirection) {
        let (Some(search), Some(status), Some(terminal)) = (
            self.ivars().terminal_search.get(),
            self.ivars().terminal_search_status.get(),
            self.active_native_terminal_view(),
        ) else {
            return;
        };
        let query = search.stringValue().to_string();
        if query.is_empty() {
            terminal.clear_search();
            status.setStringValue(&NSString::new());
            return;
        }
        let regex_mode = self
            .ivars()
            .terminal_search_regex
            .get()
            .is_some_and(|button| button.state() == NSControlStateValueOn);
        let case_sensitive = self
            .ivars()
            .terminal_search_case
            .get()
            .is_some_and(|button| button.state() == NSControlStateValueOn);
        let whole_word = self
            .ivars()
            .terminal_search_word
            .get()
            .is_some_and(|button| button.state() == NSControlStateValueOn);
        let mut pattern = if regex_mode {
            query.clone()
        } else {
            regex::escape(&query)
        };
        if whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        if !case_sensitive {
            pattern = format!("(?i:{pattern})");
        }
        match terminal.search(&pattern, direction) {
            Ok(found) => {
                status.setStringValue(&NSString::from_str(if found {
                    "Found"
                } else {
                    "No Results"
                }));
                log::debug!(
                    "native terminal search applied query_len={} direction={direction:?} found={found}",
                    query.len()
                );
            }
            Err(error) => {
                terminal.clear_search();
                status.setStringValue(&NSString::from_str("Invalid"));
                log::warn!(
                    "native terminal search invalid query_len={} regex_mode={regex_mode}: {error}",
                    query.len()
                );
            }
        }
    }

    fn layout_native_terminal_panel(&self) {
        let (Some(panel), Some(search_panel), Some(stack)) = (
            self.ivars().terminal_panel.get(),
            self.ivars().terminal_search_panel.get(),
            self.ivars().terminal_stack.get(),
        ) else {
            return;
        };
        let bounds = panel.bounds();
        let search_height = if self.ivars().terminal_search_visible.get()
            && self.ivars().terminal_search_placement.get()
                == Some(NativeTerminalPlacement::General)
        {
            38.0
        } else {
            0.0
        };
        if self.ivars().terminal_search_placement.get() == Some(NativeTerminalPlacement::General) {
            search_panel.setFrame(NSRect::new(
                NSPoint::new(0.0, (bounds.size.height - 76.0).max(0.0)),
                NSSize::new(bounds.size.width, 38.0),
            ));
        }
        stack.setFrame(NSRect::new(
            NSPoint::ZERO,
            NSSize::new(
                bounds.size.width,
                (bounds.size.height - 38.0 - search_height).max(1.0),
            ),
        ));
        self.layout_native_terminal_tabs();
    }

    fn layout_native_agent_terminal_panel(&self) {
        let (Some(agents), Some(search_panel)) = (
            self.ivars().agents.get(),
            self.ivars().terminal_search_panel.get(),
        ) else {
            return;
        };
        let bounds = agents.terminal_panel.bounds();
        let search_height = if self.ivars().terminal_search_visible.get()
            && self.ivars().terminal_search_placement.get() == Some(NativeTerminalPlacement::Agent)
        {
            38.0
        } else {
            0.0
        };
        if self.ivars().terminal_search_placement.get() == Some(NativeTerminalPlacement::Agent) {
            search_panel.setFrame(NSRect::new(
                NSPoint::new(0.0, (bounds.size.height - 38.0).max(0.0)),
                NSSize::new(bounds.size.width, 38.0),
            ));
        }
        agents.terminal_stack.setFrame(NSRect::new(
            NSPoint::ZERO,
            NSSize::new(
                bounds.size.width,
                (bounds.size.height - search_height).max(1.0),
            ),
        ));
    }

    fn set_native_terminal_visible(&self, visible: bool) {
        self.ivars().terminal_visible.set(visible);
        if let Some(item) = self.ivars().terminal_toolbar_item.get() {
            item.setToolTip(Some(&NSString::from_str(if visible {
                "Hide terminal"
            } else {
                "Show terminal"
            })));
        }
        if let Some(panel) = self.ivars().terminal_panel.get() {
            panel.setHidden(!visible);
        }
        for session in self.ivars().terminal_sessions.borrow().iter() {
            session.view.refresh_renderer_visibility();
        }
        if let Some(split) = self.ivars().content_split.get() {
            split.adjustSubviews();
        }
        self.layout_native_terminal_panel();
        if visible {
            let terminal = self
                .ivars()
                .active_general_terminal_id
                .get()
                .and_then(|id| {
                    self.ivars()
                        .terminal_sessions
                        .borrow()
                        .iter()
                        .find(|session| session.id == id)
                        .map(|session| (id, session.view.clone()))
                });
            if let Some((id, terminal)) = terminal {
                self.ivars().active_terminal_id.set(Some(id));
                if self.ivars().terminal_search_visible.get() {
                    self.show_native_terminal_search();
                } else {
                    terminal.focus_terminal();
                }
            }
        } else {
            let agent_terminal = self
                .ivars()
                .agents
                .get()
                .filter(|_| {
                    self.native_terminal_placement_is_visible(NativeTerminalPlacement::Agent)
                })
                .and_then(|_| self.ivars().active_agent_terminal_id.get())
                .and_then(|id| {
                    self.ivars()
                        .terminal_sessions
                        .borrow()
                        .iter()
                        .find(|session| session.id == id)
                        .map(|session| (id, session.view.clone()))
                });
            if let Some((id, terminal)) = agent_terminal {
                self.ivars().active_terminal_id.set(Some(id));
                if self.ivars().terminal_search_visible.get() {
                    self.show_native_terminal_search();
                } else {
                    terminal.focus_terminal();
                }
            } else if let Some(window) = self.ivars().window.get() {
                window.makeFirstResponder(None);
            }
        }
    }

    fn set_native_agent_terminal_visible(&self, visible: bool) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents.terminal_panel.setHidden(!visible);
        agents.empty.setHidden(visible);
        for session in self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .filter(|session| session.placement == NativeTerminalPlacement::Agent)
        {
            session.view.refresh_renderer_visibility();
        }
        self.layout_content();
        self.update_native_agent_terminal_card_selection();
        if visible
            && let Some(terminal) = self.ivars().active_agent_terminal_id.get().and_then(|id| {
                self.ivars()
                    .terminal_sessions
                    .borrow()
                    .iter()
                    .find(|session| session.id == id)
                    .map(|session| session.view.clone())
            })
        {
            terminal.focus_terminal();
        }
    }

    fn show_native_agent_empty_surface(&self) {
        self.set_native_agent_terminal_visible(false);
        let active_is_agent = self
            .ivars()
            .active_terminal_id
            .get()
            .and_then(|id| {
                self.ivars()
                    .terminal_sessions
                    .borrow()
                    .iter()
                    .find(|session| session.id == id)
                    .map(|session| session.placement == NativeTerminalPlacement::Agent)
            })
            .unwrap_or(false);
        if active_is_agent {
            self.ivars()
                .active_terminal_id
                .set(self.ivars().active_general_terminal_id.get());
            if self.ivars().terminal_search_visible.get() {
                if self.ivars().terminal_visible.get()
                    && self.ivars().active_general_terminal_id.get().is_some()
                {
                    self.show_native_terminal_search();
                } else {
                    self.hide_native_terminal_search();
                }
            }
        }
        self.refresh_native_agent_thread_rows();
    }

    fn request_native_terminal_close(&self, id: isize) {
        let has_active_task = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == id)
            .map(NativeTerminalSession::has_active_task);
        let Some(has_active_task) = has_active_task else {
            return;
        };
        if !has_active_task {
            self.finish_native_terminal_close(id);
            return;
        }
        let Some(window) = self.ivars().window.get().cloned() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str("Close Terminal Session?"));
        alert.setInformativeText(&NSString::from_str(
            "The running shell and any programs started from it will be stopped.",
        ));
        let close = alert.addButtonWithTitle(&NSString::from_str("Close Session"));
        close.setHasDestructiveAction(true);
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response == NSAlertFirstButtonReturn {
                delegate.finish_native_terminal_close(id);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(&window, Some(&completion));
    }

    fn reorder_native_terminal_session(&self, id: isize, delta: isize) {
        let mut sessions = self.ivars().terminal_sessions.borrow_mut();
        let Some(index) = sessions.iter().position(|session| session.id == id) else {
            return;
        };
        let placement = sessions[index].placement;
        let target = if delta < 0 {
            sessions[..index]
                .iter()
                .rposition(|session| session.placement == placement)
        } else {
            sessions[index + 1..]
                .iter()
                .position(|session| session.placement == placement)
                .map(|offset| index + 1 + offset)
        };
        let Some(target) = target else {
            return;
        };
        sessions.swap(index, target);
        drop(sessions);
        self.layout_native_terminal_tabs();
        log::debug!("native terminal session reordered id={id} from={index} to={target}");
    }

    fn close_native_terminal_remote_media(&self, id: isize, context: NativeTerminalRemoteMedia) {
        context.cancellation.cancel();
        if let Some(commands) = self.ivars().terminal_media_commands.get()
            && let Err(error) = commands.send(NativeTerminalMediaCommand::Close { session_id: id })
        {
            log::warn!("remote terminal media close request failed session={id}: {error}");
        }
    }

    fn finish_native_terminal_close(&self, id: isize) {
        self.native_provider_update_terminal_finished(id, false);
        let (mut entry, replacement, was_active) = {
            let mut sessions = self.ivars().terminal_sessions.borrow_mut();
            let Some(index) = sessions.iter().position(|session| session.id == id) else {
                return;
            };
            let entry = sessions.remove(index);
            let placement = entry.placement;
            let replacement = sessions
                .iter()
                .rev()
                .find(|session| session.placement == placement)
                .map(|session| session.id);
            let was_active = match placement {
                NativeTerminalPlacement::General => {
                    self.ivars().active_general_terminal_id.get() == Some(id)
                }
                NativeTerminalPlacement::Agent => {
                    self.ivars().active_agent_terminal_id.get() == Some(id)
                }
            };
            (entry, replacement, was_active)
        };
        let placement = entry.placement;
        if let Some(cancellation) = entry.launch_cancellation.take() {
            cancellation.cancel();
            log::info!("cancelled native agent preparation on close session={id}");
        }
        if let Some(timer) = entry.auto_close_timer.take() {
            timer.invalidate();
        }
        if let Some(context) = entry.remote_media.take() {
            self.close_native_terminal_remote_media(id, context);
        }
        if let Err(error) = entry.view.shutdown() {
            log::warn!("native terminal close failed id={id}: {error}");
        }
        entry.view.teardown_renderer();
        entry.view.removeFromSuperview();
        entry.tab.removeFromSuperview();
        self.layout_native_terminal_tabs();
        match placement {
            NativeTerminalPlacement::General => {
                if was_active {
                    self.ivars().active_general_terminal_id.set(replacement);
                    if let Some(replacement) = replacement {
                        self.activate_native_terminal_session(replacement);
                    } else {
                        self.ivars()
                            .active_terminal_id
                            .set(self.ivars().active_agent_terminal_id.get());
                        self.ivars().terminal_search_visible.set(false);
                        if let Some(panel) = self.ivars().terminal_search_panel.get() {
                            panel.setHidden(true);
                        }
                        self.set_native_terminal_visible(false);
                    }
                }
            }
            NativeTerminalPlacement::Agent => {
                if was_active {
                    self.ivars().active_agent_terminal_id.set(replacement);
                    if let Some(replacement) = replacement {
                        self.activate_native_terminal_session(replacement);
                    } else {
                        self.show_native_agent_empty_surface();
                    }
                }
            }
        }
        if placement == NativeTerminalPlacement::Agent {
            self.ivars().agent_terminal_usage.borrow_mut().remove(&id);
            if !self
                .ivars()
                .terminal_sessions
                .borrow()
                .iter()
                .any(|session| session.placement == NativeTerminalPlacement::Agent)
            {
                self.stop_native_agent_terminal_usage_timer();
            }
            self.refresh_native_agent_thread_rows();
            self.restore_agents_page_badge();
            self.request_native_cli_history(false);
        }
        log::info!("native terminal session closed id={id}");
    }

    fn shutdown_all_native_terminals(&self) {
        self.stop_native_agent_terminal_usage_timer();
        let sessions = std::mem::take(&mut *self.ivars().terminal_sessions.borrow_mut());
        for mut session in sessions {
            if let Some(cancellation) = session.launch_cancellation.take() {
                cancellation.cancel();
            }
            if let Some(timer) = session.auto_close_timer.take() {
                timer.invalidate();
            }
            if let Some(context) = session.remote_media.take() {
                self.close_native_terminal_remote_media(session.id, context);
            }
            if let Err(error) = session.view.shutdown() {
                log::warn!("native terminal shutdown failed id={}: {error}", session.id);
            }
            session.view.teardown_renderer();
            session.view.removeFromSuperview();
            session.tab.removeFromSuperview();
        }
        self.ivars().active_terminal_id.set(None);
        self.ivars().active_general_terminal_id.set(None);
        self.ivars().active_agent_terminal_id.set(None);
        self.ivars().terminal_search_visible.set(false);
        if let Some(panel) = self.ivars().terminal_search_panel.get() {
            panel.setHidden(true);
        }
        self.layout_native_terminal_tabs();
        self.set_native_agent_terminal_visible(false);
        self.set_native_terminal_visible(false);
    }

    fn prepare_for_native_shutdown(&self) {
        if self.ivars().shutdown_prepared.replace(true) {
            return;
        }
        log::info!("native AppKit shutdown preparation started");
        self.cancel_commit_message_generation();
        self.ivars().workspace_create_request_id.set(
            self.ivars()
                .workspace_create_request_id
                .get()
                .wrapping_add(1),
        );
        self.ivars().workspace_create_in_progress.set(false);
        if let Some(timer) = self.ivars().toast_timer.borrow_mut().take() {
            timer.invalidate();
        }
        if let Some(agents) = self.ivars().agents.get() {
            if let Some(cancellation) = agents.history_cancellation.borrow_mut().take() {
                cancellation.cancel();
            }
            if let Some(timer) = agents.history_refresh_timer.borrow_mut().take() {
                timer.invalidate();
            }
        }
        self.shutdown_all_native_terminals();
        if let Some(diff) = self.ivars().diff_view.get() {
            diff.teardown_renderer();
        }
        if let Some(history) = self.ivars().history.get() {
            history.diff.teardown_renderer();
        }
        if let Some(files) = self.ivars().files.get() {
            files.preview_code.teardown_renderer();
            unsafe {
                files.preview_web_content.removeAllUserScripts();
            }
        }
        self.ivars().files_monitor.borrow_mut().take();
        self.ivars().repository_background_pull.borrow_mut().take();
        self.ivars().repository_monitor.borrow_mut().take();
        log::info!("native AppKit shutdown preparation complete");
    }

    fn update_native_renderer_occlusion(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let occluded = !window
            .occlusionState()
            .contains(NSWindowOcclusionState::Visible);
        if let Some(diff) = self.ivars().diff_view.get() {
            diff.set_window_occluded(occluded);
        }
        if let Some(history) = self.ivars().history.get() {
            history.diff.set_window_occluded(occluded);
        }
        for session in self.ivars().terminal_sessions.borrow().iter() {
            session.view.set_window_occluded(occluded);
        }
        log::debug!("native Metal renderer occlusion updated occluded={occluded}");
    }

    fn schedule_controlled_app_stop(&self) {
        // SAFETY: The selector is implemented by this class and accepts the nil object supplied
        // here. Delaying it until the next main-run-loop turn lets AppKit finish returning the
        // canceled termination reply before `stop:` unwinds `NSApplication::run` back into Rust.
        unsafe {
            let _: () = msg_send![
                self,
                performSelector: sel!(stopApplicationRunLoop:),
                withObject: Option::<&AnyObject>::None,
                afterDelay: 0.0f64
            ];
        }
    }

    fn has_active_native_session(&self) -> bool {
        self.ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .any(NativeTerminalSession::has_active_task)
    }

    fn present_close_confirmation(&self) -> bool {
        if self.ivars().close_confirmation.borrow().is_some() {
            return true;
        }
        let Some(window) = self.ivars().window.get().cloned() else {
            return false;
        };

        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str("Close Craic?"));
        let agent_active = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .any(|session| {
                session.placement == NativeTerminalPlacement::Agent && session.has_active_task()
            });
        let terminal_active = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .any(|session| {
                session.placement == NativeTerminalPlacement::General && session.has_active_task()
            });
        let (message, close_label) = match (agent_active, terminal_active) {
            (true, true) => (
                "Agent and terminal sessions are active. Closing this window will stop them and any work in progress.",
                "Close and Stop Sessions",
            ),
            (true, false) => (
                "An agent session is active. Closing this window will stop it and any work in progress.",
                "Close and Stop Agent",
            ),
            (false, true) => (
                "A Terminal session is active. Closing this window will stop the shell and any programs started from it.",
                "Close and Stop Terminal",
            ),
            (false, false) => return false,
        };
        alert.setInformativeText(&NSString::from_str(message));
        let close = alert.addButtonWithTitle(&NSString::from_str(close_label));
        close.setHasDestructiveAction(true);
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));

        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            delegate.ivars().close_confirmation.borrow_mut().take();
            if response == NSAlertFirstButtonReturn {
                delegate.ivars().close_confirmed.set(true);
                log::info!("native close confirmed with active sessions");
                // AppKit may ignore `performClose:` or hold a deferred termination reply while
                // this sheet completion is still unwinding. Continue on the next main-run-loop
                // turn so both the close-button and Command-Q paths share one reliable finish.
                unsafe {
                    let _: () = msg_send![
                        &*delegate,
                        performSelector: sel!(finishConfirmedClose:),
                        withObject: Option::<&AnyObject>::None,
                        afterDelay: 0.0f64
                    ];
                }
            } else {
                delegate
                    .ivars()
                    .quit_requested_during_close_confirmation
                    .set(false);
                log::info!("native close canceled with active sessions");
            }
        });
        self.ivars().close_confirmation.replace(Some(alert.clone()));
        alert.beginSheetModalForWindow_completionHandler(&window, Some(&completion));
        true
    }

    fn confirm_open_preview_url(&self, url: Retained<NSURL>) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let display = url
            .absoluteString()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "Unknown link".to_string());
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Open Link?"));
        alert.setInformativeText(&NSString::from_str(&display));
        alert.addButtonWithTitle(&NSString::from_str("Open"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response == NSAlertFirstButtonReturn && !NSWorkspace::sharedWorkspace().openURL(&url)
            {
                delegate.present_path_action_error(
                    "Unable to Open Link",
                    "macOS could not open the selected preview link.",
                );
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn activate_native_agent_link(&self, target: LinkTarget) {
        match target {
            LinkTarget::Url(url) => {
                let Some(parsed) = NSURL::URLWithString(&NSString::from_str(&url)) else {
                    self.present_path_action_error("Unable to Open Link", "The link is invalid.");
                    return;
                };
                let scheme = parsed
                    .scheme()
                    .map(|scheme| scheme.to_string().to_ascii_lowercase())
                    .unwrap_or_default();
                if !matches!(scheme.as_str(), "http" | "https" | "mailto") {
                    self.present_path_action_error(
                        "Unable to Open Link",
                        "Only web and email links can be opened from a Codex transcript.",
                    );
                    return;
                }
                self.confirm_open_preview_url(parsed);
            }
            LinkTarget::File { path, line, column } => {
                let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
                    return;
                };
                let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
                    self.present_path_action_error(
                        "Unable to Open File",
                        "File-link navigation is unavailable for this workspace.",
                    );
                    return;
                };
                let Some(requests) = self.ivars().repository_requests.get() else {
                    self.present_path_action_error(
                        "Unable to Open File",
                        "The repository service is unavailable.",
                    );
                    return;
                };
                log::info!(
                    "native Codex file-link resolution requested workspace={workspace_id} path={path} line={line:?} column={column:?}"
                );
                if let Err(error) = requests.try_send(RepositoryRequest::ResolveAgentFileLink {
                    workspace_id,
                    handle,
                    path,
                    line,
                    column,
                }) {
                    self.present_path_action_error(
                        "Unable to Open File",
                        &format!("The file-link request could not be queued: {error}"),
                    );
                }
            }
        }
    }

    pub(crate) fn activate_native_terminal_link(&self, value: &str) {
        log::info!("native terminal link activated target={value}");
        self.activate_native_agent_link(destination_target(value));
    }

    fn apply_agent_file_link(
        &self,
        workspace_id: &str,
        line: Option<usize>,
        column: Option<usize>,
        result: Result<TerminalLinkTarget, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            log::debug!("discarding stale native Codex file link workspace={workspace_id}");
            return;
        }
        match result {
            Ok(TerminalLinkTarget::Workspace(path)) => {
                let relative = path.relative_or_empty().to_string();
                log::info!(
                    "native Codex file link opening workspace path={relative} line={line:?} column={column:?}"
                );
                self.enqueue_workspace_file_location(relative, line, column);
            }
            Ok(TerminalLinkTarget::External(path)) => {
                self.confirm_open_external_agent_file(&path.absolute, line, column);
            }
            Err(error) => {
                log::warn!("native Codex file-link resolution failed: {error}");
                self.present_path_action_error("Unable to Open File", &error);
            }
        }
    }

    fn enqueue_workspace_file_location(
        &self,
        path: String,
        line: Option<usize>,
        column: Option<usize>,
    ) {
        let Some(handle) = self.ivars().app_handle.get() else {
            log::warn!(
                "native open-file-location ignored because application actor is unavailable"
            );
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::RoutePageCommand(PageCommand {
            page: Some(PageId::new("files")),
            action: ActionId::new("open-file-location"),
            payload: serde_json::json!({
                "path": path,
                "line": line,
                "column": column,
            }),
        })) {
            log::warn!("native open-file-location queue rejected command={command:?}");
        }
    }

    fn apply_workspace_file_location(
        &self,
        path: String,
        line: Option<usize>,
        column: Option<usize>,
    ) {
        self.ivars().pending_files_path.replace(Some(path.clone()));
        self.ivars().pending_files_line.set(line);
        self.ivars().pending_files_column.set(column);
        if let (Some(files), Some(workspace_handle)) = (
            self.ivars().files.get(),
            self.ivars().workspace_handle.borrow().clone(),
        ) {
            let target = workspace_handle.workspace_files().root().join_child(&path);
            files.selected_path.replace(Some(target.clone()));
            let mut parent = target.parent();
            while let Some(path) = parent {
                if path.is_root() {
                    break;
                }
                files.expanded.borrow_mut().insert(path.clone());
                parent = path.parent();
            }
            files.dirty.set(true);
        }
        if let Some(index) = PAGE_DESCRIPTORS
            .iter()
            .position(|descriptor| descriptor.id == "files")
        {
            NSUserDefaults::standardUserDefaults()
                .setInteger_forKey(index as isize, &NSString::from_str(ACTIVE_PAGE_DEFAULT));
        }
    }

    fn confirm_open_external_agent_file(
        &self,
        absolute: &str,
        line: Option<usize>,
        column: Option<usize>,
    ) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let Some(workspace) = self
            .ivars()
            .active_workspace_id
            .borrow()
            .as_deref()
            .and_then(|active| {
                self.ivars()
                    .workspaces
                    .borrow()
                    .iter()
                    .find(|workspace| workspace.selection_id() == active)
                    .map(|workspace| workspace.workspace.clone())
            })
        else {
            return;
        };
        let (workspace_path, selected_path) =
            craic_system::workspace::external_workspace_location(absolute);
        let provider_id = workspace.provider.id();
        let display = absolute.to_string();
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Open in New Craic Window?"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "This file is outside the current workspace:\n\n{absolute}"
        )));
        alert.addButtonWithTitle(&NSString::from_str("Open"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                log::debug!("native external Codex file-link launch canceled path={display}");
                return;
            }
            if let Err(error) = launch_native_workspace_location(
                &provider_id,
                &workspace_path,
                &selected_path,
                line,
                column,
            ) {
                delegate.present_path_action_error("Unable to Open File", &error);
            } else {
                log::info!("native external Codex file-link launched path={display}");
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }
}
