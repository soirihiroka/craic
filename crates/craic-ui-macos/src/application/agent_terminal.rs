impl AppDelegate {
    fn make_native_agent_session_menu(
        &self,
        session_id: Option<isize>,
        local_id: Option<i64>,
    ) -> Retained<NSMenu> {
        let menu = NSMenu::new(self.mtm());
        menu.setAutoenablesItems(false);
        let (loaded, summarizing) = session_id
            .and_then(|id| {
                self.ivars()
                    .terminal_sessions
                    .borrow()
                    .iter()
                    .find(|session| session.id == id)
                    .map(|session| (session.view.is_active(), session.agent_summary_in_flight))
            })
            .unwrap_or((false, false));
        let has_summary = local_id.is_some_and(|id| {
            self.ivars().agents.get().is_some_and(|agents| {
                agents
                    .history_rows
                    .borrow()
                    .iter()
                    .any(|row| row.id == id && row.task_description.is_some())
            })
        });
        let status_action = if session_id.is_some() {
            sel!(showActiveAgentStatus:)
        } else {
            sel!(showAgentHistoryStatus:)
        };
        let mut actions = vec![(
            "View Status",
            status_action,
            session_id.unwrap_or(local_id.unwrap_or(-1) as isize),
            true,
        )];
        actions.push((
            if has_summary {
                "Regenerate Summary"
            } else {
                "Generate Summary"
            },
            sel!(generateAgentSummary:),
            session_id.unwrap_or(-1),
            loaded && local_id.is_some() && !summarizing,
        ));
        if let Some(id) = session_id {
            actions.push(("Unload Session", sel!(unloadAgentSession:), id, true));
        }
        if let Some(id) = local_id {
            actions.push((
                "Hide from History",
                sel!(hideAgentHistory:),
                id as isize,
                true,
            ));
        }
        for (label, action, tag, enabled) in actions {
            let item = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(label),
                    Some(action),
                    &NSString::new(),
                )
            };
            item.setTag(tag);
            item.setEnabled(enabled);
            unsafe {
                item.setTarget(Some(self));
            }
        }
        menu
    }

    fn show_native_agent_session_status(&self, session_id: Option<isize>, local_id: Option<i64>) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let session = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| {
                session_id == Some(session.id)
                    || local_id.is_some_and(|id| session.agent_history_id == Some(id))
            })
            .map(|session| {
                (
                    session.id,
                    session.agent_history_id,
                    session.title_label.clone(),
                    session.agent_provider_label.clone().unwrap_or_default(),
                    session.view.is_active(),
                    session.agent_active_state,
                )
            });
        let local_id = local_id.or_else(|| session.as_ref().and_then(|session| session.1));
        let row = match local_id
            .map(craic_agent::agent_history::lookup_session)
            .transpose()
        {
            Ok(row) => row.flatten(),
            Err(error) => {
                self.present_path_action_error("View Status Failed", &error);
                return;
            }
        };
        let mut details = Vec::new();
        if let Some((id, _, _, provider, loaded, state)) = session.as_ref() {
            details.push(format!("Loaded: {}\nLoaded Tab ID: {id}\nTerminal State: {}\nActive State: {state:?}\nProvider: {provider}",
                if *loaded { "Yes" } else { "No" }, if *loaded { "Running" } else { "Exited" }));
        } else {
            details.push("Loaded: No".to_owned());
        }
        if let Some(row) = row.as_ref() {
            details.push(format!("Craic Session ID: {}\nProvider: {}\nRestore State: {}\nCLI Session ID: {}\nWorkspace: {}\nRepo Path: {}\nCreated: {}\nUpdated: {}\nLast Seen: {}\nMetadata:\n{}",
                row.session_uuid, row.provider_id, row.restore_state.as_str(), row.cli_session_id.as_deref().unwrap_or_default(),
                row.workspace_key, row.repo_path.display(), row.created_at_ms, row.updated_at_ms, row.last_seen_at_ms,
                serde_json::from_str::<serde_json::Value>(&row.metadata_json).and_then(|metadata| serde_json::to_string_pretty(&metadata)).unwrap_or_else(|_| row.metadata_json.clone())));
        }
        let title = row
            .as_ref()
            .map(|row| row.title.as_str())
            .or_else(|| session.as_ref().map(|session| session.2.as_str()))
            .unwrap_or("Agent Session");
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Session Status"));
        alert.setInformativeText(&NSString::from_str(title));
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(500.0, 320.0)),
        );
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(false);
        text.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(11.0, 0.0)));
        text.setString(&NSString::from_str(&details.join("\n\n")));
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(500.0, 320.0)),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setDocumentView(Some(&text));
        alert.setAccessoryView(Some(&scroll));
        alert.addButtonWithTitle(&NSString::from_str("Close"));
        alert.beginSheetModalForWindow_completionHandler(window, None);
    }

    fn request_native_agent_summary(&self, id: isize, automatic: bool) {
        let (Some(identity), Some(commands), Some(cancellation)) = (
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
            self.workspace_cancellation_token(),
        ) else {
            return;
        };
        let result = (|| {
            let mut sessions = self.ivars().terminal_sessions.borrow_mut();
            let session = sessions
                .iter_mut()
                .find(|session| session.id == id && session.view.is_active())
                .ok_or_else(|| "The session terminal is not running.".to_owned())?;
            if session.agent_summary_in_flight {
                return Err("A summary is already being generated.".to_owned());
            }
            let local_id = session.agent_history_id.ok_or_else(|| {
                "Open this conversation from provider history before generating a summary."
                    .to_owned()
            })?;
            let provider = session
                .agent_provider
                .ok_or_else(|| "The agent provider is unavailable.".to_owned())?;
            let transcript = session
                .view
                .agent_transcript()
                .ok_or_else(|| "The terminal transcript is unavailable.".to_owned())?;
            if automatic
                && craic_agent::agent_history::lookup_session(local_id)?
                    .is_some_and(|row| row.task_description.is_some())
            {
                return Ok(());
            }
            commands
                .try_send(NativeAgentCommand::GenerateSummary {
                    identity,
                    session_id: id,
                    local_id,
                    provider_id: provider.provider_id().to_owned(),
                    title: session.title_label.clone(),
                    transcript,
                    automatic,
                    cancellation,
                })
                .map_err(|error| format!("The summary could not be queued: {error}"))?;
            session.agent_summary_in_flight = true;
            session.agent_summary_requested = true;
            Ok(())
        })();
        if let Err(error) = result {
            if !automatic {
                self.present_path_action_error("Generate Summary Failed", &error);
            }
        } else if !automatic {
            self.show_native_toast("Generating session summary…");
        }
    }

    fn refresh_native_terminal_agent_states(&self) {
        let mut changed = false;
        let mut summaries = Vec::new();
        {
            let mut sessions = self.ivars().terminal_sessions.borrow_mut();
            for session in sessions
                .iter_mut()
                .filter(|session| session.placement == NativeTerminalPlacement::Agent)
            {
                if !session.view.is_active() {
                    continue;
                }
                let Some(provider) = session.agent_provider else {
                    continue;
                };
                let Some((window_title, before_cursor, recent, cursor_row)) =
                    session.view.agent_content()
                else {
                    continue;
                };
                let integration = provider.shell_integration();
                if !session.agent_title_locked
                    && let Some(title) = integration.title_from_text(&before_cursor)
                {
                    session.title_label = title;
                    session
                        .title
                        .setTitle(&NSString::from_str(&session.title_label));
                    session.agent_title_locked = true;
                    session.agent_last_seen_ms = craic_agent::agent_history::unix_now_ms();
                    changed = true;
                }
                let state = integration.active_state(Some(&window_title), &|| Some(recent.clone()));
                let state = if !session.agent_title_locked && state == AgentActiveState::Idle {
                    AgentActiveState::NewChat
                } else {
                    state
                };
                if state != session.agent_active_state {
                    log::debug!(
                        "native agent state changed session={} provider={} state={state:?}",
                        session.id,
                        provider.provider_id()
                    );
                    session.agent_active_state = state;
                    changed = true;
                }
                if cursor_row >= 500
                    && !session.agent_summary_requested
                    && !session.agent_summary_in_flight
                {
                    session.agent_summary_requested = true;
                    if session.agent_history_id.is_some() {
                        summaries.push(session.id);
                    }
                }
            }
        }
        if changed {
            self.refresh_native_agent_thread_rows();
            self.restore_agents_page_badge();
        }
        for id in summaries {
            self.request_native_agent_summary(id, true);
        }
    }
}
