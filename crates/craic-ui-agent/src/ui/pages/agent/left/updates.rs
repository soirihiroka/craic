impl AgentList {
    fn check_provider_updates(&self, force: bool) {
        if self.provider_updates_checking.get()
            || self.provider_updates_running.get()
            || (!force
                && self
                    .provider_updates_checked_at
                    .get()
                    .is_some_and(|checked| checked.elapsed() < Duration::from_secs(300)))
        {
            return;
        }
        let Some(shell) = self.ctx.shell() else {
            return;
        };
        self.provider_updates_checking.set(true);
        let workspace = self.ctx.workspace_ref();
        let workspace_key = workspace.id.to_string();
        let sender = craic_ui_core::ui::command_mailbox::once({
            let generation = self.provider_updates_generation.clone();
            let requested_generation = generation.get();
            let ctx = self.ctx.clone();
            let button = self.update_button.downgrade();
            let updates = self.provider_updates.clone();
            let checking = self.provider_updates_checking.clone();
            let checked_at = self.provider_updates_checked_at.clone();
            move |result: Result<craic_agent::agent_updates::ProviderUpdateCheck, String>| {
                checking.set(false);
                if !ctx.workspace_is_current(&workspace_key)
                    || generation.get() != requested_generation
                {
                    checked_at.set(None);
                    log::debug!(
                        "discarded stale provider update check after workspace change or update"
                    );
                    return;
                }
                checked_at.set(Some(Instant::now()));
                let Some(button) = button.upgrade() else {
                    return;
                };
                match result {
                    Ok(check) => {
                        let mut available = check.updates;
                        // A temporary network failure must not erase a known update.
                        available.extend(
                            updates
                                .borrow()
                                .iter()
                                .filter(|update| !check.checked_programs.contains(&update.program))
                                .cloned(),
                        );
                        available.sort_by_key(|update| update.program);
                        if *updates.borrow() != available {
                            log::info!(
                                "provider update availability changed count={}",
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
                        button.set_tooltip_text(Some(&tooltip));
                        button.set_visible(!available.is_empty());
                        updates.replace(available);
                    }
                    Err(error) => log::warn!("provider update check failed: {error}"),
                }
            }
        });
        thread::spawn(move || {
            sender.send(craic_agent::agent_updates::check_updates(shell, workspace))
        });
    }

    fn update_providers(self: &Rc<Self>) {
        if self.provider_updates_running.get() {
            return;
        }
        let updates = self.provider_updates.borrow().clone();
        if updates.is_empty() {
            return;
        }
        let Some(shell) = self.ctx.shell() else {
            self.ctx.show_error(
                "Provider Update Failed",
                "Shell access is unavailable for this workspace.",
            );
            return;
        };
        self.provider_updates_generation
            .set(self.provider_updates_generation.get().wrapping_add(1));
        self.provider_updates_running.set(true);
        self.update_button.set_sensitive(false);
        let workspace = self.ctx.workspace_ref();
        let workspace_key = workspace.id.to_string();
        log::info!("provider updates requested count={}", updates.len());
        let sender = craic_ui_core::ui::command_mailbox::once({
            let list = Rc::downgrade(self);
            move |result: Result<crate::system::capabilities::shell::ShellCommandSpec, String>| {
                let Some(list) = list.upgrade() else { return };
                if !list.ctx.workspace_is_current(&workspace_key) {
                    list.provider_updates_running.set(false);
                    list.update_button.set_sensitive(true);
                    log::debug!("canceled provider update launch after workspace changed");
                    return;
                }
                let result = result.and_then(|command| {
                    list.ctx.run_shell_command_with_completion(&command, "Update Providers", Rc::new({
                        let list = Rc::downgrade(&list);
                        move |success| {
                            let Some(list) = list.upgrade() else { return };
                            list.provider_updates_running.set(false);
                            list.update_button.set_sensitive(true);
                            list.provider_updates_checked_at.set(None);
                            log::info!("provider update terminal completed success={success}");
                            list.ctx.show_toast(if success { "Provider updates completed." } else { "Provider updates did not complete. See the terminal for details." });
                            list.check_provider_updates(true);
                        }
                    }))
                });
                if let Err(error) = result {
                    list.provider_updates_running.set(false);
                    list.update_button.set_sensitive(true);
                    log::warn!("provider update terminal failed to open: {error}");
                    list.ctx.show_error("Provider Update Failed", &error);
                }
            }
        });
        thread::spawn(move || {
            sender.send(craic_agent::agent_updates::update_command(
                shell.as_ref(),
                &workspace,
                &updates,
            ))
        });
    }
}
