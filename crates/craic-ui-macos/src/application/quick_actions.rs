impl AppDelegate {
    fn request_native_quick_actions(&self) {
        let (workspace_id, repo_path) = match self.active_local_workspace_path() {
            Ok(workspace) => workspace,
            Err(message) => {
                self.clear_native_quick_actions(&message);
                return;
            }
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.clear_native_quick_actions("Quick Action discovery is unavailable.");
            return;
        };
        let generation = self
            .ivars()
            .quick_action_generation
            .get()
            .wrapping_add(1)
            .max(1);
        self.ivars().quick_action_generation.set(generation);
        self.ivars().quick_action_loading.set(true);
        self.ivars()
            .quick_action_workspace_id
            .replace(Some(workspace_id.clone()));
        self.ivars()
            .quick_action_repo_path
            .replace(Some(repo_path.clone()));
        self.configure_native_quick_action_group();
        if let Err(error) = requests.try_send(RepositoryRequest::LoadQuickActions {
            workspace_id: workspace_id.clone(),
            repo_path,
            generation,
            cancellation,
        }) {
            self.ivars().quick_action_loading.set(false);
            self.clear_native_quick_actions("Quick Action discovery could not be queued.");
            log::warn!(
                "native quick action discovery queue rejected workspace={workspace_id} error={error}"
            );
        }
    }

    fn apply_native_quick_actions(
        &self,
        workspace_id: &str,
        generation: u64,
        result: Result<NativeQuickActions, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || self.ivars().quick_action_generation.get() != generation
        {
            log::debug!(
                "discarding stale native quick action discovery workspace={workspace_id} generation={generation}"
            );
            return;
        }
        self.ivars().quick_action_loading.set(false);
        match result {
            Ok(mut model) => {
                for config in &mut model.configs {
                    config.selected_target_id = config
                        .selected_target_id
                        .take()
                        .filter(|selected| {
                            model.targets.iter().any(|target| target.id == *selected)
                        })
                        .or_else(|| model.targets.first().map(|target| target.id.clone()));
                }
                let target_count = model.targets.len();
                let action_count = model.configs.len();
                self.ivars().quick_action_targets.replace(model.targets);
                self.ivars().quick_action_configs.replace(model.configs);
                self.configure_native_quick_action_group();
                log::info!(
                    "native quick actions applied workspace={workspace_id} targets={target_count} actions={action_count}"
                );
            }
            Err(error) => {
                self.ivars().quick_action_targets.borrow_mut().clear();
                self.ivars().quick_action_configs.borrow_mut().clear();
                self.configure_native_quick_action_group();
                log::warn!(
                    "native quick action discovery failed workspace={workspace_id}: {error}"
                );
            }
        }
    }

    fn clear_native_quick_actions(&self, tooltip: &str) {
        self.ivars()
            .quick_action_generation
            .set(self.ivars().quick_action_generation.get().wrapping_add(1));
        self.ivars().quick_action_loading.set(false);
        self.ivars().quick_action_targets.borrow_mut().clear();
        self.ivars().quick_action_configs.borrow_mut().clear();
        self.ivars().quick_action_workspace_id.borrow_mut().take();
        self.ivars().quick_action_repo_path.borrow_mut().take();
        self.configure_native_quick_action_group();
        if let Some(group) = self.ivars().quick_action_group.get() {
            group.setToolTip(Some(&NSString::from_str(tooltip)));
        }
    }

    fn configure_native_quick_action_group(&self) {
        let Some(group) = self.ivars().quick_action_group.get() else {
            return;
        };
        let mtm = self.mtm();
        let configs = self.ivars().quick_action_configs.borrow().clone();
        let loading = self.ivars().quick_action_loading.get();
        let workspace_available = self.ivars().workspace_handle.borrow().is_some();
        let mut action_items = Vec::with_capacity(configs.len());
        let mut subitems = Vec::with_capacity(configs.len() + 1);

        for (slot, config) in configs.iter().enumerate() {
            let identifier = NSString::from_str(&format!("dev.craic.toolbar.quick-action.{slot}"));
            let item = NSMenuToolbarItem::initWithItemIdentifier(
                NSMenuToolbarItem::alloc(mtm),
                &identifier,
            );
            item.setTag(slot as isize);
            item.setBordered(true);
            item.setShowsIndicator(true);
            unsafe {
                item.setTarget(Some(self));
                item.setAction(Some(sel!(runQuickAction:)));
            }
            let selected = config.selected_target_id.as_deref().and_then(|selected| {
                self.ivars()
                    .quick_action_targets
                    .borrow()
                    .iter()
                    .find(|target| target.id == selected)
                    .cloned()
            });
            let (label, tooltip, symbol) = if loading {
                (
                    "Loading…".to_string(),
                    "Discovering project quick actions…".to_string(),
                    "hourglass".to_string(),
                )
            } else if let Some(target) = selected.as_ref() {
                (
                    target.label.clone(),
                    format!("Run quick action: {}", target.label),
                    native_quick_action_symbol(target).to_string(),
                )
            } else {
                (
                    "(empty)".to_string(),
                    "Choose a discovered project action".to_string(),
                    "play.fill".to_string(),
                )
            };
            let title = NSString::from_str(&label);
            item.setTitle(&title);
            item.setLabel(&title);
            item.setPaletteLabel(&title);
            item.setToolTip(Some(&NSString::from_str(&tooltip)));
            // Keep the split item enabled when it is empty so its menu remains available for
            // choosing the first action. The primary half reports the missing selection.
            item.setEnabled(workspace_available && !loading);
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(&symbol),
                Some(&title),
            ) {
                item.setImage(Some(&image));
            }
            self.populate_native_quick_action_menu(
                &item,
                slot,
                config.selected_target_id.as_deref(),
            );
            action_items.push(item.retain());
            subitems.push(item.into_super());
        }

        let add_identifier = NSString::from_str("dev.craic.toolbar.quick-action.add");
        let add = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), &add_identifier);
        let add_label = NSString::from_str("Add Quick Action");
        add.setLabel(&add_label);
        add.setPaletteLabel(&add_label);
        add.setToolTip(Some(&add_label));
        add.setBordered(true);
        add.setEnabled(self.active_local_workspace_path().is_ok());
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("plus"),
            Some(&add_label),
        ) {
            add.setImage(Some(&image));
        }
        unsafe {
            add.setTarget(Some(self));
            add.setAction(Some(sel!(addQuickAction:)));
        }
        subitems.push(add);

        group.setSubitems(&NSArray::from_retained_slice(&subitems));
        group.setControlRepresentation(NSToolbarItemGroupControlRepresentation::Automatic);
        group.setLabel(&NSString::from_str("Quick Actions"));
        group.setPaletteLabel(&NSString::from_str("Quick Actions"));
        group.setToolTip(Some(&NSString::from_str(
            "Run, choose, add, or remove project quick actions",
        )));
        self.ivars().quick_action_items.replace(action_items);
    }

    fn populate_native_quick_action_menu(
        &self,
        item: &NSMenuToolbarItem,
        slot: usize,
        selected_id: Option<&str>,
    ) {
        let menu = NSMenu::new(self.mtm());
        let targets = self.ivars().quick_action_targets.borrow();
        if targets.is_empty() {
            let status = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(if self.ivars().quick_action_loading.get() {
                        "Discovering Quick Actions…"
                    } else {
                        "No Quick Actions Found"
                    }),
                    None,
                    &NSString::new(),
                )
            };
            status.setEnabled(false);
        } else {
            for (target_index, target) in targets.iter().enumerate() {
                let row = unsafe {
                    menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str(&target.label),
                        Some(sel!(selectQuickAction:)),
                        &NSString::new(),
                    )
                };
                row.setTag(
                    slot.saturating_mul(targets.len())
                        .saturating_add(target_index) as isize,
                );
                row.setState(if selected_id == Some(target.id.as_str()) {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(native_quick_action_symbol(target)),
                    Some(&NSString::from_str(&target.label)),
                ) {
                    row.setImage(Some(&image));
                }
                unsafe { row.setTarget(Some(self)) };
            }
        }
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        let command = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Run Command…"),
                Some(sel!(runQuickCommand:)),
                &NSString::new(),
            )
        };
        unsafe { command.setTarget(Some(self)) };
        command.setEnabled(self.ivars().workspace_handle.borrow().is_some());
        let remove = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Remove Quick Action"),
                Some(sel!(removeQuickAction:)),
                &NSString::new(),
            )
        };
        remove.setTag(slot as isize);
        unsafe { remove.setTarget(Some(self)) };
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        let refresh = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Refresh Quick Actions"),
                Some(sel!(refreshQuickActions:)),
                &NSString::new(),
            )
        };
        unsafe { refresh.setTarget(Some(self)) };
        refresh.setEnabled(self.active_local_workspace_path().is_ok());
        item.setMenu(&menu);
    }

    fn native_quick_action_target(&self, slot: usize) -> Option<RunItem> {
        let selected_id = self
            .ivars()
            .quick_action_configs
            .borrow()
            .get(slot)?
            .selected_target_id
            .clone();
        selected_id.and_then(|selected_id| {
            self.ivars()
                .quick_action_targets
                .borrow()
                .iter()
                .find(|target| target.id == selected_id)
                .cloned()
        })
    }

    fn select_native_quick_action(&self, slot: usize, target: &RunItem) {
        let mut configs = self.ivars().quick_action_configs.borrow_mut();
        let Some(config) = configs.get_mut(slot) else {
            return;
        };
        config.selected_target_id = Some(target.id.clone());
        drop(configs);
        self.configure_native_quick_action_group();
        self.save_native_quick_action_configuration();
    }

    fn save_native_quick_action_configuration(&self) {
        let (Some(workspace_id), Some(repo_path), Some(requests)) = (
            self.ivars().quick_action_workspace_id.borrow().clone(),
            self.ivars().quick_action_repo_path.borrow().clone(),
            self.ivars().repository_requests.get(),
        ) else {
            return;
        };
        let configs = self.ivars().quick_action_configs.borrow().clone();
        if let Err(error) = requests.try_send(RepositoryRequest::SaveQuickActionConfiguration {
            workspace_id: workspace_id.clone(),
            repo_path,
            configs,
        }) {
            log::warn!(
                "native quick action configuration save queue rejected workspace={workspace_id} error={error}"
            );
        }
    }

    fn run_native_quick_action(&self, target: RunItem) {
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            self.present_path_action_error(
                "Run Failed",
                "Terminal commands are unavailable for this workspace.",
            );
            return;
        };
        let (program, arguments) = match &target.command {
            RunCommand::MakeTarget { target } => ("make", vec![target.clone()]),
            RunCommand::BunScript { script } => ("bun", vec!["run".to_string(), script.clone()]),
            RunCommand::ShellCommand { command } => ("sh", vec!["-c".to_string(), command.clone()]),
        };
        let command = match handle.terminal_command(program, &arguments) {
            Ok(command) => command,
            Err(error) => {
                self.present_path_action_error("Run Failed", &error);
                return;
            }
        };
        let files = handle.workspace_files();
        let root = files.root();
        if let Err(error) = self.spawn_native_terminal_command_with_directory(
            command,
            target.label.clone(),
            files.copy_path(&root),
            files.local_path(&root),
        ) {
            self.present_path_action_error("Run Failed", &error);
            return;
        }
        self.set_native_terminal_visible(true);
        log::info!(
            "native quick action started id={} label={}",
            target.id,
            target.label
        );
    }

}
