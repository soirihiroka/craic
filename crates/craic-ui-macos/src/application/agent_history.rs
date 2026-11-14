impl AppDelegate {
    fn ensure_native_agent_history_timer(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.history_refresh_timer.borrow().is_some() {
            return;
        }
        // SAFETY: This timer targets the retained delegate on AppKit's main run loop.
        // Shutdown invalidates it; polling only runs while the Agents page is visible.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                5.0,
                self.as_ref(),
                sel!(refreshNativeAgentHistory:),
                None,
                true,
            )
        };
        agents.history_refresh_timer.replace(Some(timer));
    }

    fn poll_native_agent_history(&self) {
        if self.ivars().shutdown_prepared.get() || !self.is_active_page("agents") {
            return;
        }
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.history_loading.get() {
            return;
        }
        let now = craic_agent::agent_history::unix_now_ms();
        if now.saturating_sub(agents.history_last_refresh_ms.get()) < 5_000 {
            return;
        }
        self.request_native_cli_history(false);
        self.request_native_provider_updates(false);
    }

    fn request_native_cli_history(&self, reset_limit: bool) {
        let (Some(agents), Some(identity), Some(commands), Some(cancellation)) = (
            self.ivars().agents.get(),
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
            self.workspace_cancellation_token(),
        ) else {
            self.complete_pending_page_service("agents", Ok(serde_json::Value::Null));
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
            self.complete_pending_page_service("agents", Ok(serde_json::Value::Null));
            return;
        };
        if reset_limit {
            agents.history_limit.set(32);
        }
        if agents.history_loading.get() {
            // Coalesce typing and refresh requests while the native readers are busy.
            agents.history_refresh_pending.set(true);
            return;
        }
        let request_id = agents.history_request_id.get().wrapping_add(1);
        agents.history_request_id.set(request_id);
        agents.history_loading.set(true);
        agents
            .history_last_refresh_ms
            .set(craic_agent::agent_history::unix_now_ms());
        if !agents.history_loaded.get() {
            agents
                .history_status
                .setStringValue(&NSString::from_str("Loading history…"));
        }
        agents.history_scope.setEnabled(false);
        let result = commands.try_send(NativeAgentCommand::LoadHistory {
            identity: identity.clone(),
            request_id,
            workspace,
            query: agents.history_query.borrow().clone(),
            tags: {
                let mut tags = agents
                    .selected_history_tags
                    .borrow()
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>();
                tags.sort();
                tags
            },
            limit: agents.history_limit.get().saturating_add(1),
            cancellation,
        });
        if let Err(error) = result {
            self.apply_native_cli_history(
                &identity,
                request_id,
                Err(format!("Unable to queue history: {error}")),
            );
        }
    }

    fn apply_native_cli_history(
        &self,
        identity: &AgentIdentity,
        request_id: u64,
        result: Result<NativeCliHistoryResult, String>,
    ) {
        if !self.agent_event_is_current(identity) {
            return;
        }
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.history_request_id.get() != request_id {
            log::debug!("discarding stale native CLI history request={request_id}");
            return;
        }
        agents.history_loading.set(false);
        if agents.history_refresh_pending.replace(false) {
            self.request_native_cli_history(false);
            return;
        }
        agents.history_loaded.set(true);
        match result {
            Ok(NativeCliHistoryResult {
                mut rows,
                tags,
                warnings,
            }) => {
                agents
                    .history_has_more
                    .set(rows.len() > agents.history_limit.get());
                rows.truncate(agents.history_limit.get());
                let count = rows.len();
                agents.history_rows.replace(rows);
                agents.history_tags.replace(tags);
                self.refresh_native_agent_history_tags();
                let status = if warnings.is_empty() {
                    format!("{count} recent sessions")
                } else {
                    format!("⚠ {count} sessions · history warnings")
                };
                agents
                    .history_status
                    .setStringValue(&NSString::from_str(&status));
                agents.history_status.setToolTip(
                    (!warnings.is_empty())
                        .then(|| NSString::from_str(&warnings.join("\n")))
                        .as_deref(),
                );
                for warning in warnings {
                    log::warn!("native agent {warning}");
                }
                self.complete_pending_page_service("agents", Ok(serde_json::Value::Null));
                log::debug!("native CLI history applied request={request_id} sessions={count}");
            }
            Err(error) => {
                agents
                    .history_status
                    .setStringValue(&NSString::from_str("⚠ Couldn’t load agent history"));
                agents
                    .history_status
                    .setToolTip(Some(&NSString::from_str(&error)));
                self.complete_pending_page_service("agents", Err(error.clone()));
                log::warn!("native CLI history failed: {error}");
            }
        }
        agents
            .history_scope
            .setEnabled(agents.history_has_more.get());
        self.refresh_native_agent_thread_rows();
        self.layout_sidebar();
    }

    fn resume_native_cli_history(&self, tag: isize) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let id = tag as i64;
        let existing = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.agent_history_id == Some(id))
            .map(|session| session.id);
        if let Some(id) = existing {
            self.activate_native_terminal_session(id);
            self.set_native_agent_terminal_visible(true);
            return;
        }
        let Some(row) = agents
            .history_rows
            .borrow()
            .iter()
            .find(|row| row.id == id)
            .cloned()
        else {
            return;
        };
        let label = match row.provider_id.as_str() {
            "codex" => "Codex CLI",
            "grok" => "Grok CLI",
            "opencode" => "OpenCode",
            _ => {
                self.present_path_action_error(
                    "Unable to Resume Agent",
                    "This provider does not support restoring sessions.",
                );
                return;
            }
        };
        if !row.restore_state.is_restorable() || row.cli_session_id.is_none() {
            self.present_path_action_error(
                "Unable to Resume Agent",
                "The original CLI session is unavailable.",
            );
            return;
        }
        self.start_native_terminal_agent(&row.provider_id, label, Some(&row));
    }

    fn refresh_native_agent_history_tags(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let menu = NSMenu::new(self.mtm());
        menu.setAutoenablesItems(false);
        let selected = agents.selected_history_tags.borrow();
        for (index, tag) in agents.history_tags.borrow().iter().enumerate() {
            let item = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(&format!("{} ({})", tag.tag, tag.session_count)),
                    Some(sel!(toggleAgentHistoryTag:)),
                    &NSString::new(),
                )
            };
            item.setTag(index as isize);
            item.setState(if selected.contains(&tag.tag) {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            unsafe {
                item.setTarget(Some(self));
            }
        }
        agents.history_tags_button.removeAllItems();
        agents
            .history_tags_button
            .addItemWithTitle(&NSString::from_str(&if selected.is_empty() {
                "Tags".to_owned()
            } else {
                format!("Tags ({})", selected.len())
            }));
        if let Some(existing) = agents.history_tags_button.menu() {
            for item in menu.itemArray().iter() {
                menu.removeItem(&item);
                existing.addItem(&item);
            }
        }
    }

    fn toggle_native_agent_history_tag(&self, sender: &NSMenuItem) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let Some(tag) = usize::try_from(sender.tag()).ok().and_then(|index| {
            agents
                .history_tags
                .borrow()
                .get(index)
                .map(|tag| tag.tag.clone())
        }) else {
            return;
        };
        let mut selected = agents.selected_history_tags.borrow_mut();
        if !selected.remove(&tag) {
            selected.insert(tag);
        }
        drop(selected);
        self.refresh_native_agent_history_tags();
        self.request_native_cli_history(true);
        self.refresh_native_agent_thread_rows();
    }

    fn native_agent_history_scrolled(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if !self.is_active_page("agents")
            || agents.history_loading.get()
            || !agents.history_has_more.get()
        {
            return;
        }
        // This document view is unflipped: zero is the bottom edge of the history.
        if agents.threads_scroll.contentView().bounds().origin.y <= 360.0 {
            agents
                .history_limit
                .set(agents.history_limit.get().saturating_add(32));
            self.request_native_cli_history(false);
        }
    }

    fn hide_native_agent_history(&self, tag: isize) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let id = tag as i64;
        match craic_agent::agent_history::hide_native_session(id) {
            Ok(()) => {
                agents.history_rows.borrow_mut().retain(|row| row.id != id);
                self.refresh_native_agent_thread_rows();
                self.request_native_cli_history(false);
            }
            Err(error) => self.present_path_action_error("Hide Session Failed", &error),
        }
    }
}
