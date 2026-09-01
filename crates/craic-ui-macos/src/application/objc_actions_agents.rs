impl AppDelegate {
    fn handle_objc_new_agent_chat(&self, _sender: &NSButton) {
        self.start_native_agent_session();
    }

    fn handle_objc_new_codex_cli(&self, _sender: &NSButton) {
        self.start_native_terminal_agent("codex", "Codex CLI");
    }

    fn handle_objc_new_agy(&self, _sender: &NSButton) {
        self.start_native_terminal_agent("agy", "AGY");
    }

    fn handle_objc_send_agent_message(&self, _sender: &NSButton) {
        self.submit_native_agent_message();
    }

    fn handle_objc_attach_agent_files(&self, _sender: &AnyObject) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setTitle(Some(&NSString::from_str("Attach to Codex")));
        panel.setPrompt(Some(&NSString::from_str("Attach")));
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(true);
        panel.setAllowedContentTypes(&NSArray::from_slice(&unsafe {
            [UTTypeImage, UTTypeAudio]
        }));
        let delegate = self.retain();
        let retained_panel = panel.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSModalResponseOK {
                return;
            }
            let Some(agents) = delegate.ivars().agents.get() else {
                return;
            };
            let mut attachments = agents.attachments.borrow_mut();
            for url in retained_panel.URLs().iter() {
                let Some(path) = url.path().map(|path| PathBuf::from(path.to_string())) else {
                    continue;
                };
                if attachments.iter().any(|attachment| attachment.path == path) {
                    continue;
                }
                let label = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                let kind = if is_native_agent_audio_path(&path) {
                    NativeAgentAttachmentKind::Audio
                } else {
                    NativeAgentAttachmentKind::Image
                };
                attachments.push(NativeAgentAttachment { path, label, kind });
            }
            drop(attachments);
            delegate.refresh_native_agent_attachments();
            delegate.refresh_agent_controls();
        });
        panel.beginSheetModalForWindow_completionHandler(window, &completion);
    }

    fn handle_objc_reference_agent_file(&self, _sender: &NSMenuItem) {
        self.choose_native_agent_reference(false);
    }

    fn handle_objc_reference_agent_folder(&self, _sender: &NSMenuItem) {
        self.choose_native_agent_reference(true);
    }

    fn handle_objc_clear_agent_attachments(&self, _sender: &NSButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents.attachments.borrow_mut().clear();
        self.refresh_native_agent_attachments();
        self.refresh_agent_controls();
    }

    fn handle_objc_stop_agent_turn(&self, _sender: &NSButton) {
        let Some(identity) = self.active_agent_identity() else {
            return;
        };
        let Some(commands) = self.ivars().agent_commands.get() else {
            return;
        };
        if let Err(error) = commands.try_send(NativeAgentCommand::Interrupt { identity }) {
            self.present_path_action_error(
                "Unable to Stop Codex",
                &format!("The stop request could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_select_agent_model(&self, sender: &NSPopUpButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.selector_updates_suppressed.get() {
            return;
        }
        let Ok(index) = usize::try_from(sender.indexOfSelectedItem()) else {
            return;
        };
        let Some(model) = agents
            .model_options
            .borrow()
            .get(index)
            .map(|option| option.id.clone())
        else {
            return;
        };
        let (Some(identity), Some(commands)) = (
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
        ) else {
            return;
        };
        agents.selected_model.replace(Some(model.clone()));
        save_native_string_default(AGENT_MODEL_DEFAULT, &model);
        if let Err(error) =
            commands.try_send(NativeAgentCommand::SetModel { identity, model })
        {
            self.present_path_action_error(
                "Unable to Change Codex Model",
                &format!("The model update could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_select_agent_reasoning(&self, sender: &NSPopUpButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.selector_updates_suppressed.get() {
            return;
        }
        let Ok(index) = usize::try_from(sender.indexOfSelectedItem()) else {
            return;
        };
        let Some(reasoning) = agents
            .reasoning_options
            .borrow()
            .get(index)
            .map(|option| option.id.clone())
        else {
            return;
        };
        let (Some(identity), Some(commands)) = (
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
        ) else {
            return;
        };
        agents.selected_reasoning.replace(Some(reasoning.clone()));
        save_native_string_default(AGENT_REASONING_DEFAULT, &reasoning);
        if let Err(error) = commands.try_send(NativeAgentCommand::SetReasoning {
            identity,
            reasoning,
        }) {
            self.present_path_action_error(
                "Unable to Change Reasoning Effort",
                &format!("The reasoning update could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_select_agent_personality(&self, sender: &NSPopUpButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.selector_updates_suppressed.get() {
            return;
        }
        let Ok(index) = usize::try_from(sender.indexOfSelectedItem()) else {
            return;
        };
        let Some(personality) = agents
            .personality_options
            .borrow()
            .get(index)
            .map(|option| option.id.clone())
        else {
            return;
        };
        let (Some(identity), Some(commands)) = (
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
        ) else {
            return;
        };
        agents
            .selected_personality
            .replace(Some(personality.clone()));
        save_native_string_default(AGENT_PERSONALITY_DEFAULT, &personality);
        if let Err(error) = commands.try_send(NativeAgentCommand::SetPersonality {
            identity,
            personality,
        }) {
            self.present_path_action_error(
                "Unable to Change Codex Personality",
                &format!("The personality update could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_select_agent_service_tier(&self, sender: &NSPopUpButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.selector_updates_suppressed.get() {
            return;
        }
        let Ok(index) = usize::try_from(sender.indexOfSelectedItem()) else {
            return;
        };
        let Some(service_tier) = agents
            .service_tier_options
            .borrow()
            .get(index)
            .map(|option| option.id.clone())
        else {
            return;
        };
        let (Some(identity), Some(commands)) = (
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
        ) else {
            return;
        };
        agents
            .selected_service_tier
            .replace(Some(service_tier.clone()));
        save_native_string_default(AGENT_SERVICE_TIER_DEFAULT, &service_tier);
        if let Err(error) = commands.try_send(NativeAgentCommand::SetServiceTier {
            identity,
            service_tier,
        }) {
            self.present_path_action_error(
                "Unable to Change Response Speed",
                &format!("The response-speed update could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_select_agent_permissions(&self, sender: &NSPopUpButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents.selector_updates_suppressed.get() {
            return;
        }
        let Ok(index) = usize::try_from(sender.indexOfSelectedItem()) else {
            return;
        };
        let Some(permissions) = agents
            .permission_options
            .borrow()
            .get(index)
            .map(|option| option.id.clone())
        else {
            return;
        };
        let (Some(identity), Some(commands)) = (
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
        ) else {
            return;
        };
        agents
            .selected_permissions
            .replace(Some(permissions.clone()));
        save_native_string_default(AGENT_PERMISSIONS_DEFAULT, &permissions);
        if let Err(error) = commands.try_send(NativeAgentCommand::SetPermissions {
            identity,
            permissions,
        }) {
            self.present_path_action_error(
                "Unable to Change Codex Permissions",
                &format!("The permission update could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_resume_agent_thread(&self, sender: &NSButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        self.show_native_agent_app_surface();
        let Ok(index) = usize::try_from(sender.tag()) else {
            return;
        };
        let Some(thread_id) = agents
            .threads
            .borrow()
            .get(index)
            .map(|thread| thread.id.clone())
        else {
            return;
        };
        if agents.active_thread_id.borrow().as_deref() == Some(thread_id.as_str()) {
            return;
        }
        let (Some(identity), Some(commands)) = (
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
        ) else {
            return;
        };
        let archived = agents
            .threads
            .borrow()
            .get(index)
            .is_some_and(|thread| thread.archived);
        let command = if archived {
            NativeAgentCommand::UnarchiveThread {
                identity,
                thread_id,
            }
        } else {
            NativeAgentCommand::Resume {
                identity,
                thread_id,
            }
        };
        if let Err(error) = commands.try_send(command) {
            self.present_path_action_error(
                "Unable to Open Codex Chat",
                &format!("The chat could not be opened: {error}"),
            );
        }
    }

    fn handle_objc_filter_agent_threads(&self, sender: &NSSearchField) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents
            .history_query
            .replace(sender.stringValue().to_string());
        self.request_native_agent_thread_filter();
    }

    fn handle_objc_select_agent_thread_scope(&self, sender: &NSPopUpButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents.history_archived.set(sender.indexOfSelectedItem() == 1);
        self.request_native_agent_thread_filter();
    }

    fn handle_objc_rename_agent_thread(&self, sender: &NSMenuItem) {
        let Some(thread) = self.native_agent_thread_for_tag(sender.tag()) else {
            return;
        };
        self.prompt_native_agent_thread_rename(thread);
    }

    fn handle_objc_archive_agent_thread(&self, sender: &NSMenuItem) {
        let Some(thread) = self.native_agent_thread_for_tag(sender.tag()) else {
            return;
        };
        self.send_native_agent_thread_operation(NativeAgentCommand::ArchiveThread {
            identity: match self.active_agent_identity() {
                Some(identity) => identity,
                None => return,
            },
            thread_id: thread.id,
        });
    }

    fn handle_objc_unarchive_agent_thread(&self, sender: &NSMenuItem) {
        let Some(thread) = self.native_agent_thread_for_tag(sender.tag()) else {
            return;
        };
        self.send_native_agent_thread_operation(NativeAgentCommand::UnarchiveThread {
            identity: match self.active_agent_identity() {
                Some(identity) => identity,
                None => return,
            },
            thread_id: thread.id,
        });
    }

    fn handle_objc_delete_agent_thread(&self, sender: &NSMenuItem) {
        let Some(thread) = self.native_agent_thread_for_tag(sender.tag()) else {
            return;
        };
        self.confirm_native_agent_thread_delete(thread);
    }

    fn handle_objc_show_agent_thread_history(&self, _sender: &NSMenuItem) {
        self.show_agents_search();
    }

    fn handle_objc_show_agent_thread_goal(&self, _sender: &NSMenuItem) {
        self.prompt_native_agent_thread_goal();
    }

    fn handle_objc_run_agent_shell_command(&self, _sender: &NSMenuItem) {
        self.prompt_native_agent_shell_command();
    }

    fn handle_objc_show_agent_background_terminals(&self, _sender: &NSMenuItem) {
        self.run_native_agent_tool(NativeAgentToolAction::BackgroundTerminals);
    }

    fn handle_objc_show_agent_skills(&self, _sender: &NSMenuItem) {
        self.run_native_agent_tool(NativeAgentToolAction::Skills);
    }

    fn handle_objc_show_agent_mcp_servers(&self, _sender: &NSMenuItem) {
        self.run_native_agent_tool(NativeAgentToolAction::McpServers);
    }

    fn handle_objc_show_agent_apps(&self, _sender: &NSMenuItem) {
        self.run_native_agent_tool(NativeAgentToolAction::Apps);
    }

    fn handle_objc_show_agent_plugins(&self, _sender: &NSMenuItem) {
        self.run_native_agent_tool(NativeAgentToolAction::Plugins);
    }

    fn handle_objc_show_agent_experimental_features(&self, _sender: &NSMenuItem) {
        self.run_native_agent_tool(NativeAgentToolAction::ExperimentalFeatures);
    }

    fn handle_objc_show_agent_account_usage(&self, _sender: &NSMenuItem) {
        self.run_native_agent_tool(NativeAgentToolAction::AccountUsage);
    }

    fn handle_objc_fork_active_agent_thread(&self, _sender: &NSMenuItem) {
        self.run_native_agent_thread_action(NativeAgentThreadAction::Fork);
    }

    fn handle_objc_compact_active_agent_thread(&self, _sender: &NSMenuItem) {
        self.run_native_agent_thread_action(NativeAgentThreadAction::Compact);
    }

    fn handle_objc_start_agent_review(&self, _sender: &NSMenuItem) {
        self.prompt_native_agent_review();
    }

    fn handle_objc_rollback_active_agent_thread(&self, _sender: &NSMenuItem) {
        self.run_native_agent_thread_action(NativeAgentThreadAction::Rollback);
    }

    fn handle_objc_archive_active_agent_thread(&self, _sender: &NSMenuItem) {
        let (Some(identity), Some(thread_id)) = (
            self.active_agent_identity(),
            self.ivars()
                .agents
                .get()
                .and_then(|agents| agents.active_thread_id.borrow().clone()),
        ) else {
            return;
        };
        self.send_native_agent_thread_operation(NativeAgentCommand::ArchiveThread {
            identity,
            thread_id,
        });
    }

    fn handle_objc_open_agent_changes(&self, _sender: &NSMenuItem) {
        let Some(handle) = self.ivars().app_handle.get() else {
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::ActivatePage(PageId::new("changes"))) {
            log::warn!("agent Changes page activation rejected command={command:?}");
        } else if let Some(index) = PAGE_DESCRIPTORS
            .iter()
            .position(|descriptor| descriptor.id == "changes")
        {
            NSUserDefaults::standardUserDefaults()
                .setInteger_forKey(index as isize, &NSString::from_str(ACTIVE_PAGE_DEFAULT));
        }
    }

    fn handle_objc_select_page(&self, sender: &NSToolbarItemGroup) {
        let Some((index, descriptor)) = usize::try_from(sender.selectedIndex())
            .ok()
            .and_then(|index| PAGE_DESCRIPTORS.get(index).map(|descriptor| (index, descriptor)))
        else {
            return;
        };
        let Some(handle) = self.ivars().app_handle.get() else {
            log::warn!("page activation ignored because application actor is unavailable");
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::ActivatePage(descriptor.page_id())) {
            log::warn!("page activation queue rejected command={command:?}");
        } else {
            NSUserDefaults::standardUserDefaults().setInteger_forKey(
                index as isize,
                &NSString::from_str(ACTIVE_PAGE_DEFAULT),
            );
        }
    }
}
