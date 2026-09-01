impl AppDelegate {
    fn set_changes_operation_progress(&self, message: &str) {
        if let Some(check) = self.ivars().select_all_check.get() {
            check.setEnabled(false);
        }
        if let Some(label) = self.ivars().select_all_label.get() {
            label.setStringValue(&NSString::from_str(message));
        }
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.set_repository_available(false);
        }
    }

    fn show_native_toast(&self, message: &str) {
        let (Some(toast), Some(label)) = (self.ivars().toast.get(), self.ivars().toast_label.get())
        else {
            return;
        };
        if let Some(timer) = self.ivars().toast_timer.borrow_mut().take() {
            timer.invalidate();
        }
        label.setStringValue(&NSString::from_str(message));
        label.setToolTip(Some(&NSString::from_str(message)));
        toast.setAccessibilityLabel(Some(&NSString::from_str(message)));
        toast.setHidden(false);
        // SAFETY: The timer runs on AppKit's main run loop and targets a selector implemented by
        // this retained application delegate. Replacing a toast invalidates the previous timer.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                4.0,
                self.as_ref(),
                sel!(hideNativeToast:),
                None,
                false,
            )
        };
        self.ivars().toast_timer.replace(Some(timer));
        log::debug!("native toast presented message={message}");
    }

    fn changes_operation_failed(&self, heading: &str, message: &str) {
        self.refresh_selection_header();
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.set_repository_available(self.ivars().git_handle.borrow().is_some());
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(heading));
        alert.setInformativeText(&NSString::from_str(message));
        alert.addButtonWithTitle(&NSString::from_str("OK"));
        alert.beginSheetModalForWindow_completionHandler(window, None);
        log::warn!("native changes operation failed heading={heading}: {message}");
    }

    fn active_agent_identity(&self) -> Option<AgentIdentity> {
        let agents = self.ivars().agents.get()?;
        Some(AgentIdentity {
            workspace_id: self.ivars().active_workspace_id.borrow().clone()?,
            generation: agents.generation.get(),
        })
    }

    fn native_agent_thread_for_tag(&self, tag: isize) -> Option<NativeAgentThreadSummary> {
        let index = usize::try_from(tag).ok()?;
        self.ivars()
            .agents
            .get()?
            .threads
            .borrow()
            .get(index)
            .cloned()
    }

    fn send_native_agent_thread_operation(&self, command: NativeAgentCommand) {
        let Some(commands) = self.ivars().agent_commands.get() else {
            self.present_path_action_error(
                "Unable to Update Codex Chat",
                "The native Codex service is unavailable.",
            );
            return;
        };
        if let Err(error) = commands.try_send(command) {
            self.present_path_action_error(
                "Unable to Update Codex Chat",
                &format!("The chat update could not be queued: {error}"),
            );
        }
    }

    fn run_native_agent_thread_action(&self, action: NativeAgentThreadAction) {
        let Some(identity) = self.active_agent_identity() else {
            return;
        };
        self.send_native_agent_thread_operation(NativeAgentCommand::RunActiveThreadAction {
            identity,
            action,
        });
    }

    fn run_native_agent_tool(&self, action: NativeAgentToolAction) {
        let Some(identity) = self.active_agent_identity() else {
            return;
        };
        self.send_native_agent_thread_operation(NativeAgentCommand::RunTool { identity, action });
    }

    fn request_native_agent_thread_filter(&self) {
        let (Some(agents), Some(identity), Some(commands)) = (
            self.ivars().agents.get(),
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
        ) else {
            return;
        };
        let query = agents.history_query.borrow().clone();
        let archived = agents.history_archived.get();
        if let Err(error) = commands.try_send(NativeAgentCommand::FilterThreads {
            identity,
            query,
            archived,
        }) {
            self.present_path_action_error(
                "Unable to Filter Codex Chats",
                &format!("The history filter could not be queued: {error}"),
            );
        }
    }

    fn prompt_native_agent_thread_goal(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Thread Goal"));
        alert.setInformativeText(&NSString::from_str(
            "View, replace, or clear the goal tracked by this Codex thread.",
        ));
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 26.0)),
        );
        input.setPlaceholderString(Some(&NSString::from_str("Goal objective")));
        alert.setAccessoryView(Some(&input));
        alert.addButtonWithTitle(&NSString::from_str("Set Goal"));
        alert.addButtonWithTitle(&NSString::from_str("View Current"));
        let clear = alert.addButtonWithTitle(&NSString::from_str("Clear"));
        clear.setHasDestructiveAction(true);
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            let action = match response - NSAlertFirstButtonReturn {
                0 => {
                    let objective = completion_input.stringValue().to_string().trim().to_owned();
                    if objective.is_empty() {
                        delegate.present_path_action_error(
                            "Unable to Set Goal",
                            "Enter a goal objective before saving.",
                        );
                        return;
                    }
                    NativeAgentToolAction::SetThreadGoal(objective)
                }
                1 => NativeAgentToolAction::ViewThreadGoal,
                2 => NativeAgentToolAction::ClearThreadGoal,
                _ => return,
            };
            delegate.run_native_agent_tool(action);
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        alert.window().makeFirstResponder(Some(&input));
    }

    fn prompt_native_agent_shell_command(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str("Run Thread Shell Command"));
        alert.setInformativeText(&NSString::from_str(
            "This command runs through the thread's shell with full workspace access, outside the Codex sandbox.",
        ));
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 26.0)),
        );
        input.setPlaceholderString(Some(&NSString::from_str("Shell command")));
        alert.setAccessoryView(Some(&input));
        let run = alert.addButtonWithTitle(&NSString::from_str("Run Command"));
        run.setHasDestructiveAction(true);
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let command = completion_input.stringValue().to_string().trim().to_owned();
            if command.is_empty() {
                delegate.present_path_action_error(
                    "Unable to Run Command",
                    "Enter a shell command to run.",
                );
                return;
            }
            delegate.run_native_agent_tool(NativeAgentToolAction::RunShellCommand(command));
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        alert.window().makeFirstResponder(Some(&input));
    }

    fn present_native_agent_background_terminals(
        &self,
        terminals: Vec<NativeAgentBackgroundTerminal>,
    ) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Background Terminals"));
        if terminals.is_empty() {
            alert.setInformativeText(&NSString::from_str(
                "No background terminals are running for this thread.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.beginSheetModalForWindow_completionHandler(window, None);
            return;
        }
        alert.setInformativeText(&NSString::from_str(
            "Select a command to stop, or stop every background command owned by this thread.",
        ));
        let picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(460.0, 28.0)),
            false,
        );
        for terminal in &terminals {
            picker.addItemWithTitle(&NSString::from_str(&terminal.command));
            if let Some(item) = picker.lastItem() {
                item.setToolTip(Some(&NSString::from_str(&terminal.detail)));
            }
        }
        alert.setAccessoryView(Some(&picker));
        let selected = alert.addButtonWithTitle(&NSString::from_str("Stop Selected"));
        selected.setHasDestructiveAction(true);
        let all = alert.addButtonWithTitle(&NSString::from_str("Stop All"));
        all.setHasDestructiveAction(true);
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion_picker = picker.clone();
        let completion = RcBlock::new(move |response| match response - NSAlertFirstButtonReturn {
            0 => {
                if let Ok(index) = usize::try_from(completion_picker.indexOfSelectedItem())
                    && let Some(terminal) = terminals.get(index)
                {
                    delegate.run_native_agent_tool(NativeAgentToolAction::StopBackgroundTerminal(
                        terminal.process_id.clone(),
                    ));
                }
            }
            1 => delegate.run_native_agent_tool(NativeAgentToolAction::StopAllBackgroundTerminals),
            _ => {}
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn present_native_agent_skills(&self, skills: Vec<NativeAgentSkillOption>) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Add a Skill"));
        if skills.is_empty() {
            alert.setInformativeText(&NSString::from_str(
                "No enabled skills are available for this workspace.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.beginSheetModalForWindow_completionHandler(window, None);
            return;
        }
        alert.setInformativeText(&NSString::from_str(
            "Select a Codex skill to include with the next message.",
        ));
        let picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(460.0, 28.0)),
            false,
        );
        for skill in &skills {
            picker.addItemWithTitle(&NSString::from_str(&skill.name));
            if let Some(item) = picker.lastItem() {
                let tooltip = if skill.description.is_empty() {
                    skill.path.as_str()
                } else {
                    skill.description.as_str()
                };
                item.setToolTip(Some(&NSString::from_str(tooltip)));
            }
        }
        alert.setAccessoryView(Some(&picker));
        alert.addButtonWithTitle(&NSString::from_str("Add Skill"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion_picker = picker.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let Ok(index) = usize::try_from(completion_picker.indexOfSelectedItem()) else {
                return;
            };
            let Some(skill) = skills.get(index) else {
                return;
            };
            let Some(agents) = delegate.ivars().agents.get() else {
                return;
            };
            let path = PathBuf::from(&skill.path);
            if !agents.attachments.borrow().iter().any(|attachment| {
                attachment.kind == NativeAgentAttachmentKind::Skill && attachment.path == path
            }) {
                agents.attachments.borrow_mut().push(NativeAgentAttachment {
                    path,
                    label: skill.name.clone(),
                    kind: NativeAgentAttachmentKind::Skill,
                });
            }
            delegate.refresh_native_agent_attachments();
            delegate.refresh_agent_controls();
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn present_native_agent_experimental_features(
        &self,
        features: Vec<NativeAgentExperimentalFeature>,
    ) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Experimental Features"));
        if features.is_empty() {
            alert.setInformativeText(&NSString::from_str(
                "No configurable experimental features are available.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.beginSheetModalForWindow_completionHandler(window, None);
            return;
        }
        alert.setInformativeText(&NSString::from_str(
            "Changes apply to the current Codex App Server process.",
        ));
        let visible_rows = features.len().min(10);
        let document_height = (features.len() as f64 * 34.0).max(1.0);
        let document = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(460.0, document_height)),
        );
        let mut toggles = Vec::with_capacity(features.len());
        for (index, feature) in features.iter().enumerate() {
            let title = if feature.description.is_empty() {
                feature.label.clone()
            } else {
                format!("{} — {}", feature.label, feature.description)
            };
            let toggle = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str(&title),
                    None,
                    None,
                    self.mtm(),
                )
            };
            toggle.setState(if feature.enabled {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            toggle.setToolTip(Some(&NSString::from_str(&feature.description)));
            toggle.setFrame(NSRect::new(
                NSPoint::new(0.0, document_height - (index as f64 + 1.0) * 34.0),
                NSSize::new(460.0, 30.0),
            ));
            document.addSubview(&toggle);
            toggles.push((feature.name.clone(), feature.enabled, toggle));
        }
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(460.0, visible_rows as f64 * 34.0),
            ),
        );
        scroll.setBorderType(NSBorderType::BezelBorder);
        scroll.setHasVerticalScroller(features.len() > visible_rows);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&document));
        alert.setAccessoryView(Some(&scroll));
        alert.addButtonWithTitle(&NSString::from_str("Apply"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let enablement = toggles
                .iter()
                .filter_map(|(name, initial, toggle)| {
                    let enabled = toggle.state() == NSControlStateValueOn;
                    (enabled != *initial).then(|| (name.clone(), enabled))
                })
                .collect::<BTreeMap<_, _>>();
            if !enablement.is_empty() {
                delegate.run_native_agent_tool(NativeAgentToolAction::SetExperimentalFeatures(
                    enablement,
                ));
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn prompt_native_agent_review(&self) {
        let (Some(window), Some(identity)) =
            (self.ivars().window.get(), self.active_agent_identity())
        else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Start Code Review"));
        alert.setInformativeText(&NSString::from_str(
            "Choose what Codex should review and whether to keep the review in this chat.",
        ));

        let accessory = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 104.0)),
        );
        let target = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm()),
            NSRect::new(NSPoint::new(0.0, 76.0), NSSize::new(420.0, 28.0)),
            false,
        );
        for title in [
            "Uncommitted Changes",
            "Against Base Branch",
            "Commit",
            "Custom Instructions",
        ] {
            target.addItemWithTitle(&NSString::from_str(title));
        }
        target.setToolTip(Some(&NSString::from_str("Review target")));
        accessory.addSubview(&target);

        let value = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::new(0.0, 40.0), NSSize::new(420.0, 26.0)),
        );
        value.setPlaceholderString(Some(&NSString::from_str(
            "Base branch, commit SHA, or custom instructions",
        )));
        accessory.addSubview(&value);

        let delivery = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm()),
            NSRect::new(NSPoint::new(0.0, 4.0), NSSize::new(420.0, 28.0)),
            false,
        );
        delivery.addItemWithTitle(&NSString::from_str("Inline Review"));
        delivery.addItemWithTitle(&NSString::from_str("Detached Review Thread"));
        delivery.setToolTip(Some(&NSString::from_str("Review delivery")));
        accessory.addSubview(&delivery);
        alert.setAccessoryView(Some(&accessory));
        alert.addButtonWithTitle(&NSString::from_str("Start Review"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));

        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let input = value.stringValue().to_string().trim().to_owned();
            let target = match target.indexOfSelectedItem() {
                0 => NativeAgentReviewTarget::UncommittedChanges,
                1 if !input.is_empty() => NativeAgentReviewTarget::BaseBranch(input),
                2 if !input.is_empty() => NativeAgentReviewTarget::Commit(input),
                3 if !input.is_empty() => NativeAgentReviewTarget::Custom(input),
                _ => {
                    delegate.present_path_action_error(
                        "Unable to Start Review",
                        "Enter the base branch, commit SHA, or custom review instructions.",
                    );
                    return;
                }
            };
            delegate.send_native_agent_thread_operation(NativeAgentCommand::StartReview {
                identity: identity.clone(),
                target,
                detached: delivery.indexOfSelectedItem() == 1,
            });
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn prompt_native_agent_thread_rename(&self, thread: NativeAgentThreadSummary) {
        let (Some(window), Some(identity)) =
            (self.ivars().window.get(), self.active_agent_identity())
        else {
            return;
        };
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(360.0, 26.0)),
        );
        input.setStringValue(&NSString::from_str(&thread.title));
        input.setPlaceholderString(Some(&NSString::from_str("Thread name")));
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Rename Codex Thread"));
        alert.setInformativeText(&NSString::from_str(
            "Choose the name shown in Codex thread history.",
        ));
        alert.setAccessoryView(Some(&input));
        alert.addButtonWithTitle(&NSString::from_str("Rename"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let name = completion_input.stringValue().to_string();
            if name.trim().is_empty() {
                delegate.present_path_action_error(
                    "Unable to Rename Codex Thread",
                    "A Codex thread name cannot be empty.",
                );
                return;
            }
            delegate.send_native_agent_thread_operation(NativeAgentCommand::RenameThread {
                identity: identity.clone(),
                thread_id: thread.id.clone(),
                name,
            });
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        alert.window().makeFirstResponder(Some(&input));
        // SAFETY: The retained field is the active sheet's editable control.
        unsafe { input.selectText(None) };
    }

    fn confirm_native_agent_thread_delete(&self, thread: NativeAgentThreadSummary) {
        let (Some(window), Some(identity)) =
            (self.ivars().window.get(), self.active_agent_identity())
        else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str("Delete Codex Thread?"));
        alert.setInformativeText(&NSString::from_str(
            "This permanently deletes the thread and its Codex history.",
        ));
        let delete = alert.addButtonWithTitle(&NSString::from_str("Delete Thread"));
        delete.setHasDestructiveAction(true);
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response == NSAlertFirstButtonReturn {
                delegate.send_native_agent_thread_operation(NativeAgentCommand::DeleteThread {
                    identity: identity.clone(),
                    thread_id: thread.id.clone(),
                });
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn start_native_agent_session(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        self.show_native_agent_app_surface();
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            self.present_path_action_error(
                "Unable to Start Codex",
                "Open a workspace before starting an agent session.",
            );
            return;
        };
        let workspace = self
            .ivars()
            .workspaces
            .borrow()
            .iter()
            .find(|workspace| workspace.selection_id() == workspace_id)
            .map(|workspace| workspace.workspace.clone());
        let Some(workspace) = workspace else {
            self.present_path_action_error(
                "Unable to Start Codex",
                "The active workspace is no longer available.",
            );
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            self.present_path_action_error(
                "Unable to Start Codex",
                "Workspace cancellation is unavailable.",
            );
            return;
        };
        let generation = agents.generation.get().wrapping_add(1);
        agents.generation.set(generation);
        agents.transcript_items.borrow_mut().clear();
        self.clear_native_agent_transcript_images();
        agents.threads.borrow_mut().clear();
        agents.active_thread_id.borrow_mut().take();
        agents.transcript_table.reloadData();
        agents.composer.setString(&NSString::new());
        agents.new_chat.setEnabled(false);
        agents.empty.setHidden(true);
        agents.composer_scroll.setHidden(true);
        agents.send.setHidden(true);
        agents.stop.setHidden(true);
        agents.history_query.borrow_mut().clear();
        agents.history_archived.set(false);
        agents.history_search.setStringValue(&NSString::new());
        agents.history_scope.selectItemAtIndex(0);
        agents.history_search.setEnabled(false);
        agents.history_scope.setEnabled(false);
        agents.model.setEnabled(false);
        agents.reasoning.setEnabled(false);
        agents.personality.setEnabled(false);
        agents.service_tier.setEnabled(false);
        agents.permissions.setEnabled(false);
        self.apply_native_agent_usage(None);
        agents
            .status
            .setStringValue(&NSString::from_str("Connecting to Codex…"));
        agents.state.set(NativeAgentState::Connecting);
        unsafe { agents.spinner.startAnimation(None) };
        agents.spinner.setHidden(false);
        self.set_page_badge("agents", NativePageBadge::Indicator);
        let identity = AgentIdentity {
            workspace_id,
            generation,
        };
        let Some(commands) = self.ivars().agent_commands.get() else {
            self.apply_native_agent_state(
                &identity,
                NativeAgentState::Closed,
                Some("The native Codex service is unavailable."),
            );
            return;
        };
        if let Err(error) = commands.try_send(NativeAgentCommand::Start {
            identity: identity.clone(),
            workspace,
            cancellation,
            model: agents.selected_model.borrow().clone(),
            reasoning: agents.selected_reasoning.borrow().clone(),
            personality: agents.selected_personality.borrow().clone(),
            service_tier: agents.selected_service_tier.borrow().clone(),
            permissions: agents.selected_permissions.borrow().clone(),
        }) {
            self.apply_native_agent_state(
                &identity,
                NativeAgentState::Closed,
                Some(&format!("Unable to queue Codex startup: {error}")),
            );
        }
    }

    fn submit_native_agent_message(&self) {
        let (Some(agents), Some(identity), Some(commands)) = (
            self.ivars().agents.get(),
            self.active_agent_identity(),
            self.ivars().agent_commands.get(),
        ) else {
            return;
        };
        let text = agents.composer.string().to_string();
        let attachments = agents.attachments.borrow().clone();
        if (text.trim().is_empty() && attachments.is_empty())
            || agents.state.get() != NativeAgentState::Ready
        {
            return;
        }
        if let Err(error) = commands.try_send(NativeAgentCommand::Send {
            identity,
            text,
            attachments,
        }) {
            self.present_path_action_error(
                "Unable to Send Message",
                &format!("The Codex message could not be queued: {error}"),
            );
            return;
        }
        agents.composer.setString(&NSString::new());
        agents.attachments.borrow_mut().clear();
        self.refresh_native_agent_attachments();
        self.refresh_agent_controls();
    }

    fn apply_native_agent_event(&self, event: NativeAgentEvent) {
        match event {
            NativeAgentEvent::Cleared => self.reset_native_agent_ui(),
            NativeAgentEvent::State {
                identity,
                state,
                detail,
            } => self.apply_native_agent_state(&identity, state, detail.as_deref()),
            NativeAgentEvent::ThreadReady {
                identity,
                thread_id,
                title,
            } => {
                if self.agent_event_is_current(&identity) {
                    if let Some(agents) = self.ivars().agents.get() {
                        agents.active_thread_id.replace(Some(thread_id.clone()));
                        let title = title
                            .as_deref()
                            .filter(|title| !title.trim().is_empty())
                            .unwrap_or("New Codex chat");
                        agents.title.setStringValue(&NSString::from_str(title));
                        self.refresh_native_agent_thread_rows();
                    }
                    log::info!(
                        "native Codex thread ready workspace={} generation={} thread_id={}",
                        identity.workspace_id,
                        identity.generation,
                        thread_id
                    );
                }
            }
            NativeAgentEvent::Upsert { identity, item } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                let Some(agents) = self.ivars().agents.get() else {
                    return;
                };
                let item_id = item.id.clone();
                let mut items = agents.transcript_items.borrow_mut();
                let image_changed = items
                    .iter()
                    .find(|existing| existing.id == item_id)
                    .is_some_and(|existing| existing.image != item.image);
                if let Some(existing) = items.iter_mut().find(|existing| existing.id == item_id) {
                    *existing = item;
                } else {
                    items.push(item);
                }
                let image_item = items
                    .iter()
                    .find(|candidate| candidate.id == item_id)
                    .cloned();
                drop(items);
                if image_changed {
                    self.remove_native_agent_transcript_image(&item_id);
                }
                if let Some(item) = image_item.as_ref() {
                    self.request_native_agent_transcript_image(&identity, item);
                }
                self.render_native_agent_transcript();
            }
            NativeAgentEvent::Models {
                identity,
                options,
                selected,
            } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                let Some(agents) = self.ivars().agents.get() else {
                    return;
                };
                agents.model_options.replace(options);
                agents.selected_model.replace(selected.clone());
                save_native_optional_default(AGENT_MODEL_DEFAULT, selected.as_deref());
                self.refresh_native_agent_selectors();
            }
            NativeAgentEvent::ReasoningOptions {
                identity,
                options,
                selected,
            } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                let Some(agents) = self.ivars().agents.get() else {
                    return;
                };
                agents.reasoning_options.replace(options);
                agents.selected_reasoning.replace(selected.clone());
                save_native_optional_default(AGENT_REASONING_DEFAULT, selected.as_deref());
                self.refresh_native_agent_selectors();
            }
            NativeAgentEvent::PersonalityOptions {
                identity,
                options,
                selected,
            } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                let Some(agents) = self.ivars().agents.get() else {
                    return;
                };
                agents.personality_options.replace(options);
                agents.selected_personality.replace(selected.clone());
                save_native_optional_default(AGENT_PERSONALITY_DEFAULT, selected.as_deref());
                self.refresh_native_agent_selectors();
            }
            NativeAgentEvent::ServiceTierOptions {
                identity,
                options,
                selected,
            } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                let Some(agents) = self.ivars().agents.get() else {
                    return;
                };
                agents.service_tier_options.replace(options);
                agents.selected_service_tier.replace(selected.clone());
                save_native_optional_default(AGENT_SERVICE_TIER_DEFAULT, selected.as_deref());
                self.refresh_native_agent_selectors();
            }
            NativeAgentEvent::PermissionProfiles {
                identity,
                options,
                selected,
            } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                let Some(agents) = self.ivars().agents.get() else {
                    return;
                };
                agents.permission_options.replace(options);
                agents.selected_permissions.replace(selected.clone());
                save_native_optional_default(AGENT_PERMISSIONS_DEFAULT, selected.as_deref());
                self.refresh_native_agent_selectors();
            }
            NativeAgentEvent::SettingApplied { identity, setting } => {
                if self.agent_event_is_current(&identity) {
                    log::info!(
                        "native Codex setting applied workspace={} generation={} setting={setting:?}",
                        identity.workspace_id,
                        identity.generation
                    );
                }
            }
            NativeAgentEvent::Usage { identity, usage } => {
                if self.agent_event_is_current(&identity) {
                    self.apply_native_agent_usage(usage.as_ref());
                }
            }
            NativeAgentEvent::Threads { identity, threads } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                if let Some(agents) = self.ivars().agents.get() {
                    agents.threads.replace(threads);
                    self.refresh_native_agent_thread_rows();
                }
            }
            NativeAgentEvent::TranscriptCleared { identity } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                if let Some(agents) = self.ivars().agents.get() {
                    agents.transcript_items.borrow_mut().clear();
                    self.clear_native_agent_transcript_images();
                    agents.transcript_table.reloadData();
                    agents.transcript_scroll.setHidden(true);
                    agents
                        .empty
                        .setStringValue(&NSString::from_str("Loading conversation…"));
                    agents.empty.setHidden(false);
                }
            }
            NativeAgentEvent::ThreadClosed { identity, message } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                if let Some(agents) = self.ivars().agents.get() {
                    agents.active_thread_id.borrow_mut().take();
                    agents.transcript_items.borrow_mut().clear();
                    self.clear_native_agent_transcript_images();
                    agents.transcript_table.reloadData();
                    agents.transcript_scroll.setHidden(true);
                    agents
                        .title
                        .setStringValue(&NSString::from_str("New Codex chat"));
                    agents.empty.setStringValue(&NSString::from_str(&message));
                    agents.empty.setHidden(false);
                    agents.new_chat.setEnabled(true);
                    self.refresh_native_agent_thread_rows();
                }
            }
            NativeAgentEvent::ThreadOperationApplied {
                identity,
                thread_id,
                operation,
            } => {
                if self.agent_event_is_current(&identity) {
                    log::info!(
                        "native Codex thread operation applied workspace={} generation={} thread_id={} operation={operation:?}",
                        identity.workspace_id,
                        identity.generation,
                        thread_id
                    );
                }
            }
            NativeAgentEvent::Request { identity, request } => {
                if self.agent_event_is_current(&identity) {
                    self.ivars()
                        .agent_pending_request_keys
                        .borrow_mut()
                        .insert(request.key.clone());
                    self.present_native_agent_request(identity, request);
                }
            }
            NativeAgentEvent::RequestResolved {
                identity,
                request_key,
            } => {
                if self.agent_event_is_current(&identity) {
                    self.ivars()
                        .agent_pending_request_keys
                        .borrow_mut()
                        .remove(&request_key);
                    self.ivars()
                        .agent_request_multiline_inputs
                        .borrow_mut()
                        .remove(&request_key);
                    if let Some(alert) = self
                        .ivars()
                        .agent_request_alerts
                        .borrow_mut()
                        .remove(&request_key)
                    {
                        alert.window().close();
                    }
                }
            }
            NativeAgentEvent::BackgroundTerminals {
                identity,
                terminals,
            } => {
                if self.agent_event_is_current(&identity) {
                    self.present_native_agent_background_terminals(terminals);
                }
            }
            NativeAgentEvent::Skills { identity, skills } => {
                if self.agent_event_is_current(&identity) {
                    self.present_native_agent_skills(skills);
                }
            }
            NativeAgentEvent::ExperimentalFeatures { identity, features } => {
                if self.agent_event_is_current(&identity) {
                    self.present_native_agent_experimental_features(features);
                }
            }
        }
    }

    fn agent_event_is_current(&self, identity: &AgentIdentity) -> bool {
        self.ivars().active_workspace_id.borrow().as_deref() == Some(identity.workspace_id.as_str())
            && self
                .ivars()
                .agents
                .get()
                .is_some_and(|agents| agents.generation.get() == identity.generation)
    }

    fn present_native_agent_request(
        &self,
        identity: AgentIdentity,
        request: NativeAgentPendingRequest,
    ) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        if self
            .ivars()
            .agent_request_alerts
            .borrow()
            .contains_key(&request.key)
        {
            return;
        }
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(&request.title));
        let informative_text = if request.multiline_text {
            request
                .text_placeholder
                .as_deref()
                .map(|placeholder| {
                    if request.message.trim().is_empty() {
                        placeholder.to_owned()
                    } else {
                        format!("{}\n\n{placeholder}", request.message)
                    }
                })
                .unwrap_or_else(|| request.message.clone())
        } else {
            request.message.clone()
        };
        alert.setInformativeText(&NSString::from_str(&informative_text));

        let single_line_input: Option<Retained<NSTextField>> =
            (request.allows_text && !request.multiline_text).then(|| {
                let input: Retained<NSTextField> = if request.secret {
                    NSSecureTextField::initWithFrame(
                        NSSecureTextField::alloc(self.mtm()),
                        NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 26.0)),
                    )
                    .into_super()
                } else {
                    NSTextField::initWithFrame(
                        NSTextField::alloc(self.mtm()),
                        NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 26.0)),
                    )
                };
                if let Some(placeholder) = request.text_placeholder.as_deref() {
                    input.setPlaceholderString(Some(&NSString::from_str(placeholder)));
                }
                alert.setAccessoryView(Some(&input));
                input
            });
        let multiline_input: Option<Retained<NSTextView>> = request.multiline_text.then(|| {
            let frame = NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 112.0));
            let input = NSTextView::initWithFrame(NSTextView::alloc(self.mtm()), frame);
            input.setEditable(true);
            input.setSelectable(true);
            input.setRichText(false);
            input.setDrawsBackground(true);
            if let Some(font) = NSFont::userFixedPitchFontOfSize(12.0) {
                input.setFont(Some(&font));
            }
            input.setTextContainerInset(NSSize::new(8.0, 8.0));
            input.setDelegate(Some(ProtocolObject::from_ref(self)));

            let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(self.mtm()), frame);
            scroll.setBorderType(NSBorderType::BezelBorder);
            scroll.setDrawsBackground(true);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDocumentView(Some(&input));
            alert.setAccessoryView(Some(&scroll));
            input
        });

        let mut responses = Vec::new();
        let mut text_submit = None;
        let text_button = request.allows_text.then(|| {
            let button = alert.addButtonWithTitle(&NSString::from_str(if request.multiline_text {
                "Return Output"
            } else {
                "Submit"
            }));
            if request.multiline_text {
                button.setEnabled(false);
            }
            text_submit = Some(button);
            responses.push(NativeAgentRequestResponse::Text(String::new()));
            responses.len() - 1
        });
        for option in &request.options {
            let button = alert.addButtonWithTitle(&NSString::from_str(&option.label));
            button.setHasDestructiveAction(option.destructive);
            responses.push(NativeAgentRequestResponse::Choice(option.value.clone()));
        }
        if !request.multiline_text
            && (request.allows_text
                || !request
                    .options
                    .iter()
                    .any(|option| matches!(option.value.as_str(), "decline" | "cancel")))
        {
            let button = alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            button.setHasDestructiveAction(true);
            responses.push(NativeAgentRequestResponse::Cancel);
        }

        let request_key = request.key.clone();
        let completion_key = request.key.clone();
        let completion_input = single_line_input.clone();
        let completion_multiline_input = multiline_input.clone();
        let retry_request = request.clone();
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            delegate
                .ivars()
                .agent_request_alerts
                .borrow_mut()
                .remove(&completion_key);
            delegate
                .ivars()
                .agent_request_multiline_inputs
                .borrow_mut()
                .remove(&completion_key);
            if !delegate.agent_event_is_current(&identity) {
                return;
            }
            let Some(index) = usize::try_from(response - NSAlertFirstButtonReturn).ok() else {
                return;
            };
            let Some(mut response) = responses.get(index).cloned() else {
                return;
            };
            if text_button == Some(index) {
                if let Some(input) = completion_multiline_input.as_ref() {
                    response = NativeAgentRequestResponse::Text(
                        input.string().to_string().trim().to_owned(),
                    );
                } else if let Some(input) = completion_input.as_ref() {
                    response = NativeAgentRequestResponse::Text(input.stringValue().to_string());
                }
            }
            let Some(commands) = delegate.ivars().agent_commands.get() else {
                delegate.present_native_agent_response_queue_error(
                    identity.clone(),
                    retry_request.clone(),
                    "The native Codex command service is unavailable.",
                );
                return;
            };
            if let Err(error) = commands.try_send(NativeAgentCommand::Respond {
                identity: identity.clone(),
                request_key: completion_key.clone(),
                response,
            }) {
                delegate.present_native_agent_response_queue_error(
                    identity.clone(),
                    retry_request.clone(),
                    &format!("The response could not be queued: {error}"),
                );
            }
        });
        self.ivars()
            .agent_request_alerts
            .borrow_mut()
            .insert(request_key, alert.clone());
        if let (Some(input), Some(submit)) = (multiline_input.as_ref(), text_submit) {
            self.ivars()
                .agent_request_multiline_inputs
                .borrow_mut()
                .insert(request.key.clone(), (input.clone(), submit));
        }
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        if let Some(input) = multiline_input {
            alert.window().makeFirstResponder(Some(&input));
        } else if let Some(input) = single_line_input {
            alert.window().makeFirstResponder(Some(&input));
        }
    }

    fn present_native_agent_response_queue_error(
        &self,
        identity: AgentIdentity,
        request: NativeAgentPendingRequest,
        message: &str,
    ) {
        if !self.agent_event_is_current(&identity)
            || !self
                .ivars()
                .agent_pending_request_keys
                .borrow()
                .contains(&request.key)
        {
            return;
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Critical);
        alert.setMessageText(&NSString::from_str("Unable to Answer Codex"));
        alert.setInformativeText(&NSString::from_str(message));
        alert.addButtonWithTitle(&NSString::from_str("OK"));
        let delegate = self.retain();
        let completion = RcBlock::new(move |_| {
            if delegate.agent_event_is_current(&identity)
                && delegate
                    .ivars()
                    .agent_pending_request_keys
                    .borrow()
                    .contains(&request.key)
            {
                delegate.present_native_agent_request(identity.clone(), request.clone());
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

}
