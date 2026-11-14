impl AppDelegate {
    fn request_native_provider_updates(&self, force: bool) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.updates_checking.get()
            || agents.updates_running.get()
            || (!force
                && agents
                    .updates_checked_at
                    .get()
                    .is_some_and(|checked| checked.elapsed() < Duration::from_secs(300)))
        {
            return;
        }
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
        let generation = agents.updates_generation.get();
        agents.updates_checking.set(true);
        agents.updates_check_forced.set(force);
        if let Err(error) = commands.try_send(NativeAgentCommand::CheckProviderUpdates {
            identity,
            generation,
            workspace,
            cancellation,
        }) {
            agents.updates_checking.set(false);
            agents.updates_check_forced.set(false);
            log::warn!("native provider update check could not be queued: {error}");
            if force {
                self.present_path_action_error("Provider Update Check Failed", &error.to_string());
            }
        } else if force {
            self.show_native_toast("Checking for provider updates…");
        }
    }

    fn apply_native_provider_updates(
        &self,
        identity: &AgentIdentity,
        generation: u64,
        result: Result<craic_agent::agent_updates::ProviderUpdateCheck, String>,
    ) {
        if !self.agent_event_is_current(identity) {
            return;
        }
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.updates_generation.get() != generation {
            log::debug!("discarded stale native provider update check generation={generation}");
            return;
        }
        agents.updates_checking.set(false);
        let force = agents.updates_check_forced.replace(false);
        agents
            .updates_checked_at
            .set(Some(std::time::Instant::now()));
        match result {
            Ok(check) => {
                let incomplete = check.checked_programs.len() < NATIVE_AGENT_PROVIDERS.len();
                let mut available = check.updates;
                // A temporary version/release check failure must retain a known update.
                available.extend(
                    agents
                        .provider_updates
                        .borrow()
                        .iter()
                        .filter(|update| !check.checked_programs.contains(&update.program))
                        .cloned(),
                );
                available.sort_by_key(|update| update.program);
                if *agents.provider_updates.borrow() != available {
                    log::info!(
                        "native provider update availability changed count={}",
                        available.len()
                    );
                }
                let tooltip = available
                    .iter()
                    .map(|update| {
                        format!(
                            "{}: {} → {}",
                            update.label, update.installed_version, update.latest_version
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                agents
                    .update_button
                    .setToolTip(Some(&NSString::from_str(&tooltip)));
                agents
                    .update_button
                    .setEnabled(!agents.updates_running.get());
                agents.update_button.setHidden(available.is_empty());
                let count = available.len();
                agents.provider_updates.replace(available);
                self.layout_sidebar();
                if force {
                    self.show_native_toast(match (count > 0, incomplete) {
                        (true, true) => {
                            "Provider updates are available. Some update checks failed."
                        }
                        (true, false) => "Provider updates are available.",
                        (false, true) => "Some provider update checks failed. Try again later.",
                        (false, false) => "No provider updates are available.",
                    });
                }
            }
            Err(error) => {
                log::warn!("native provider update check failed: {error}");
                if force {
                    self.present_path_action_error("Provider Update Check Failed", &error);
                }
            }
        }
    }

    fn update_native_agent_providers(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.updates_running.get() {
            return;
        }
        let updates = agents.provider_updates.borrow().clone();
        if updates.is_empty() {
            self.request_native_provider_updates(true);
            return;
        }
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
        let generation = agents.updates_generation.get().wrapping_add(1);
        agents.updates_generation.set(generation);
        agents.updates_running.set(true);
        agents.updates_checking.set(false);
        agents.updates_check_forced.set(false);
        agents.update_button.setEnabled(false);
        log::info!("native provider updates requested count={}", updates.len());
        if let Err(error) = commands.try_send(NativeAgentCommand::PrepareProviderUpdates {
            identity,
            generation,
            workspace,
            updates,
            cancellation,
        }) {
            agents.updates_running.set(false);
            agents.update_button.setEnabled(true);
            self.present_path_action_error(
                "Provider Update Failed",
                &format!("The update could not be queued: {error}"),
            );
        }
    }

    fn finish_native_provider_update_start(
        &self,
        identity: &AgentIdentity,
        generation: u64,
        result: Result<ShellCommandSpec, String>,
    ) {
        if !self.agent_event_is_current(identity) {
            return;
        }
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.updates_generation.get() != generation {
            return;
        }
        let result = result.and_then(|command| {
            self.spawn_native_terminal_command(command, "Update Providers".to_owned())
        });
        match result {
            Ok(()) => {
                agents
                    .update_terminal_id
                    .set(self.ivars().active_general_terminal_id.get());
                self.set_native_terminal_visible(true);
                self.show_native_toast("Provider updates are running in the terminal.");
            }
            Err(error) => {
                agents.updates_running.set(false);
                agents.update_button.setEnabled(true);
                log::warn!("native provider update terminal failed to open: {error}");
                self.present_path_action_error("Provider Update Failed", &error);
            }
        }
    }

    fn native_provider_update_terminal_finished(&self, id: isize, success: bool) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.update_terminal_id.get() != Some(id) {
            return;
        }
        agents.update_terminal_id.set(None);
        agents.updates_running.set(false);
        agents.update_button.setEnabled(true);
        agents.updates_checked_at.set(None);
        log::info!("native provider update terminal completed success={success}");
        if self
            .workspace_cancellation_token()
            .is_none_or(|token| token.is_cancelled())
        {
            return;
        }
        self.show_native_toast(if success {
            "Provider updates completed."
        } else {
            "Provider updates did not complete. See the terminal for details."
        });
        // Clearing the timestamp forces a fresh check without replacing the completion toast.
        self.request_native_provider_updates(false);
    }

    fn reset_native_provider_updates(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents
            .updates_generation
            .set(agents.updates_generation.get().wrapping_add(1));
        agents.updates_checking.set(false);
        agents.updates_check_forced.set(false);
        agents.updates_running.set(false);
        agents.updates_checked_at.set(None);
        agents.update_terminal_id.set(None);
        agents.provider_updates.borrow_mut().clear();
        agents.update_button.setEnabled(true);
        agents.update_button.setHidden(true);
    }
}
