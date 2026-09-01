impl AppDelegate {
    fn apply_native_agent_state(
        &self,
        identity: &AgentIdentity,
        state: NativeAgentState,
        detail: Option<&str>,
    ) {
        if !self.agent_event_is_current(identity) {
            return;
        }
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents.state.set(state);
        agents.new_chat.setEnabled(!matches!(
            state,
            NativeAgentState::Connecting
                | NativeAgentState::Initializing
                | NativeAgentState::Stopping
        ));
        let fallback = match state {
            NativeAgentState::Connecting => "Connecting to Codex…",
            NativeAgentState::Initializing => "Initializing Codex…",
            NativeAgentState::Ready => "Ready",
            NativeAgentState::Running => "Codex is working…",
            NativeAgentState::Stopping => "Stopping…",
            NativeAgentState::Closed => "Codex session closed",
        };
        let detail = detail.unwrap_or(fallback);
        let no_active_thread = agents.active_thread_id.borrow().is_none();
        agents.status.setStringValue(&NSString::from_str(detail));
        let spinning = matches!(
            state,
            NativeAgentState::Connecting
                | NativeAgentState::Initializing
                | NativeAgentState::Running
                | NativeAgentState::Stopping
        );
        if spinning {
            unsafe { agents.spinner.startAnimation(None) };
        } else {
            unsafe { agents.spinner.stopAnimation(None) };
        }
        agents.spinner.setHidden(!spinning);
        if state == NativeAgentState::Closed && no_active_thread {
            agents.empty.setStringValue(&NSString::from_str(detail));
            agents.empty.setHidden(false);
        } else {
            agents.empty.setHidden(true);
        }
        let can_compose = state == NativeAgentState::Ready;
        let can_browse_history =
            matches!(state, NativeAgentState::Ready | NativeAgentState::Running);
        agents.history_search.setEnabled(can_browse_history);
        agents.history_scope.setEnabled(can_browse_history);
        agents
            .thread_actions
            .setEnabled(can_browse_history && !no_active_thread);
        agents
            .tools
            .setEnabled(can_browse_history && !no_active_thread);
        agents.composer_scroll.setHidden(matches!(
            state,
            NativeAgentState::Connecting
                | NativeAgentState::Initializing
                | NativeAgentState::Closed
        ));
        agents.attach.setHidden(agents.composer_scroll.isHidden());
        if agents.composer_scroll.isHidden() {
            agents.attachment_tokens.setHidden(true);
            agents.clear_attachments.setHidden(true);
        } else {
            self.refresh_native_agent_attachments();
        }
        agents.send.setHidden(agents.composer_scroll.isHidden());
        agents.stop.setHidden(!matches!(
            state,
            NativeAgentState::Running | NativeAgentState::Stopping
        ));
        agents.composer.setEditable(can_compose);
        self.refresh_native_agent_selectors();
        self.refresh_native_agent_thread_rows();
        self.set_page_badge(
            "agents",
            if spinning {
                NativePageBadge::Indicator
            } else {
                NativePageBadge::None
            },
        );
        self.refresh_agent_controls();
    }

    fn refresh_agent_controls(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let ready = agents.state.get() == NativeAgentState::Ready;
        let active_workspace = self.ivars().active_workspace_id.borrow().clone();
        agents.send.setEnabled(
            ready
                && (!agents.composer.string().to_string().trim().is_empty()
                    || !agents.attachments.borrow().is_empty()),
        );
        agents
            .attach
            .setEnabled(ready && active_workspace.is_some());
        agents
            .attach
            .setToolTip(Some(&NSString::from_str("Add context")));
        agents
            .clear_attachments
            .setEnabled(ready && !agents.attachments.borrow().is_empty());
        agents.stop.setEnabled(matches!(
            agents.state.get(),
            NativeAgentState::Running | NativeAgentState::Stopping
        ));
    }

    fn choose_native_agent_reference(&self, folder: bool) {
        let (Some(window), Some(workspace_id)) = (
            self.ivars().window.get(),
            self.ivars().active_workspace_id.borrow().clone(),
        ) else {
            return;
        };
        let workspace = self
            .ivars()
            .workspaces
            .borrow()
            .iter()
            .find(|workspace| workspace.selection_id() == workspace_id)
            .cloned();
        let Some(workspace) = workspace else {
            return;
        };
        if let Some(root) = workspace.local_path() {
            let panel = NSOpenPanel::openPanel(self.mtm());
            panel.setTitle(Some(&NSString::from_str(if folder {
                "Reference Workspace Folder"
            } else {
                "Reference Workspace File"
            })));
            panel.setPrompt(Some(&NSString::from_str("Reference")));
            panel.setCanChooseFiles(!folder);
            panel.setCanChooseDirectories(folder);
            panel.setAllowsMultipleSelection(false);
            panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
                &root.to_string_lossy(),
            ))));
            let delegate = self.retain();
            let retained_panel = panel.clone();
            let completion = RcBlock::new(move |response| {
                if response != NSModalResponseOK {
                    return;
                }
                let Some(path) = retained_panel
                    .URL()
                    .and_then(|url| url.path())
                    .map(|path| PathBuf::from(path.to_string()))
                else {
                    return;
                };
                if !path.starts_with(&root) {
                    delegate.present_path_action_error(
                        "Unable to Reference Item",
                        "Choose an item inside the active workspace.",
                    );
                    return;
                }
                delegate.append_native_agent_reference(path);
            });
            panel.beginSheetModalForWindow_completionHandler(window, &completion);
            return;
        }

        let root = workspace.workspace.path.trim_end_matches('/').to_owned();
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(if folder {
            "Reference Remote Workspace Folder"
        } else {
            "Reference Remote Workspace File"
        }));
        alert.setInformativeText(&NSString::from_str(
            "Enter an absolute path inside the active SSH workspace.",
        ));
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 26.0)),
        );
        input.setStringValue(&NSString::from_str(&format!("{root}/")));
        alert.setAccessoryView(Some(&input));
        alert.addButtonWithTitle(&NSString::from_str("Reference"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let path = completion_input.stringValue().to_string().trim().to_owned();
            if path.is_empty() || (path != root && !path.starts_with(&format!("{root}/"))) {
                delegate.present_path_action_error(
                    "Unable to Reference Item",
                    "Enter a path inside the active SSH workspace.",
                );
                return;
            }
            delegate.append_native_agent_reference(PathBuf::from(path));
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        alert.window().makeFirstResponder(Some(&input));
    }

    fn append_native_agent_reference(&self, path: PathBuf) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let mut attachments = agents.attachments.borrow_mut();
        if attachments.iter().any(|attachment| attachment.path == path) {
            return;
        }
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        attachments.push(NativeAgentAttachment {
            path,
            label,
            kind: NativeAgentAttachmentKind::Mention,
        });
        drop(attachments);
        self.refresh_native_agent_attachments();
        self.refresh_agent_controls();
    }

    fn refresh_native_agent_attachments(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let attachments = agents.attachments.borrow();
        let tokens = NSArray::from_retained_slice(
            &attachments
                .iter()
                .map(|attachment| NSString::from_str(&attachment.label))
                .collect::<Vec<_>>(),
        );
        unsafe { agents.attachment_tokens.setObjectValue(Some(&tokens)) };
        let hidden = attachments.is_empty();
        agents.attachment_tokens.setHidden(hidden);
        agents.clear_attachments.setHidden(hidden);
        let detail = attachments
            .iter()
            .map(|attachment| attachment.path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        agents.attachment_tokens.setToolTip(
            (!detail.is_empty())
                .then(|| NSString::from_str(&detail))
                .as_deref(),
        );
    }

    fn refresh_native_agent_selectors(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents.selector_updates_suppressed.set(true);
        populate_agent_selector(
            &agents.model,
            &agents.model_options.borrow(),
            agents.selected_model.borrow().as_deref(),
            "No models available",
        );
        populate_agent_selector(
            &agents.reasoning,
            &agents.reasoning_options.borrow(),
            agents.selected_reasoning.borrow().as_deref(),
            "No reasoning options",
        );
        populate_agent_selector(
            &agents.personality,
            &agents.personality_options.borrow(),
            agents.selected_personality.borrow().as_deref(),
            "No personalities available",
        );
        populate_agent_selector(
            &agents.service_tier,
            &agents.service_tier_options.borrow(),
            agents.selected_service_tier.borrow().as_deref(),
            "Standard response speed",
        );
        populate_agent_selector(
            &agents.permissions,
            &agents.permission_options.borrow(),
            agents.selected_permissions.borrow().as_deref(),
            "No permission profiles",
        );
        agents.selector_updates_suppressed.set(false);
        let adjustable = matches!(
            agents.state.get(),
            NativeAgentState::Ready | NativeAgentState::Running
        );
        agents
            .model
            .setEnabled(adjustable && !agents.model_options.borrow().is_empty());
        agents
            .reasoning
            .setEnabled(adjustable && !agents.reasoning_options.borrow().is_empty());
        agents
            .personality
            .setEnabled(adjustable && !agents.personality_options.borrow().is_empty());
        agents
            .service_tier
            .setEnabled(adjustable && !agents.service_tier_options.borrow().is_empty());
        agents
            .permissions
            .setEnabled(adjustable && !agents.permission_options.borrow().is_empty());
    }

    fn apply_native_agent_usage(&self, usage: Option<&NativeAgentTokenUsage>) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let Some(usage) = usage else {
            agents
                .usage
                .setStringValue(&NSString::from_str("Context unavailable"));
            agents.usage.setToolTip(Some(&NSString::from_str(
                "Token usage is not available yet",
            )));
            agents.usage_progress.setDoubleValue(0.0);
            agents.usage_progress.setToolTip(Some(&NSString::from_str(
                "Token usage is not available yet",
            )));
            return;
        };

        const BASELINE_TOKENS: u64 = 12_000;
        let remaining = usage.context_limit.filter(|limit| *limit > 0).map(|limit| {
            let effective_window = limit.saturating_sub(BASELINE_TOKENS);
            let used = usage.last_total_tokens.saturating_sub(BASELINE_TOKENS);
            let remaining = effective_window.saturating_sub(used);
            let fraction = if effective_window == 0 {
                0.0
            } else {
                (remaining as f64 / effective_window as f64).clamp(0.0, 1.0)
            };
            (remaining, effective_window, fraction)
        });
        let label = remaining.map_or_else(
            || "Context remaining: unknown".to_owned(),
            |(_, _, fraction)| format!("{:.0}% context remaining", fraction * 100.0),
        );
        let remaining_detail = remaining.map_or_else(
            || "Context remaining: unknown".to_owned(),
            |(remaining, effective_window, fraction)| {
                format!(
                    "Context remaining: {remaining} / {effective_window} tokens ({:.0}%)",
                    fraction * 100.0
                )
            },
        );
        let detail = format!(
            "{remaining_detail}\nActive context: {}{}\n\nCumulative usage\nInput: {}\nCache write input: {}\nCached input: {}\nOutput: {}\nReasoning output: {}\nTotal: {}",
            usage.last_total_tokens,
            usage
                .context_limit
                .map_or_else(String::new, |limit| format!(" / {limit}")),
            usage.input_tokens,
            usage.cache_write_input_tokens,
            usage.cached_input_tokens,
            usage.output_tokens,
            usage.reasoning_output_tokens,
            usage.total_tokens,
        );
        agents.usage.setStringValue(&NSString::from_str(&label));
        agents.usage.setToolTip(Some(&NSString::from_str(&detail)));
        agents
            .usage_progress
            .setDoubleValue(remaining.map_or(0.0, |(_, _, fraction)| fraction));
        agents
            .usage_progress
            .setToolTip(Some(&NSString::from_str(&detail)));
    }

    fn refresh_native_agent_thread_rows(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let active_agent_id = self.ivars().active_agent_terminal_id.get();
        let agent_surface_visible = !agents.terminal_panel.isHidden();
        let usage = self.ivars().agent_terminal_usage.borrow();
        let agent_sessions = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .filter(|session| session.placement == NativeTerminalPlacement::Agent)
            .map(|session| {
                (
                    session.id,
                    session.title_label.clone(),
                    session.agent_provider_label.clone(),
                    session.view.is_active(),
                    usage.get(&session.id).copied(),
                )
            })
            .collect::<Vec<_>>();
        drop(usage);
        let subviews = agents.threads_document.subviews();
        for index in 0..subviews.count() {
            subviews.objectAtIndex(index).removeFromSuperview();
        }
        agents.terminal_cards.borrow_mut().clear();
        let threads = agents.threads.borrow();
        let viewport = agents.threads_scroll.contentSize();
        let threads_clip = agents.threads_scroll.contentView();
        let previous_document_height = agents.threads_document.frame().size.height;
        let previous_scroll_y = threads_clip.bounds().origin.y;
        let previous_top = (previous_document_height - viewport.height).max(0.0);
        let keep_scrolled_to_top = previous_document_height <= viewport.height + 1.0
            || previous_scroll_y >= previous_top - 1.0;
        let total_rows = agent_sessions.len() + threads.len();
        let thread_previews = threads
            .iter()
            .map(|thread| {
                [
                    thread.smart_summary.as_deref(),
                    Some(thread.preview.as_str()),
                ]
                .into_iter()
                .flatten()
                .map(str::trim)
                .find(|preview| !preview.is_empty() && *preview != thread.title.trim())
                .unwrap_or("")
            })
            .collect::<Vec<_>>();
        let content_height = agent_sessions.len() as f64 * AGENT_TERMINAL_ROW_HEIGHT
            + thread_previews
                .iter()
                .map(|preview| {
                    if preview.is_empty() {
                        AGENT_THREAD_COMPACT_ROW_HEIGHT
                    } else {
                        AGENT_THREAD_PREVIEW_ROW_HEIGHT
                    }
                })
                .sum::<f64>();
        let height = content_height.max(viewport.height);
        agents
            .threads_document
            .setFrameSize(NSSize::new(viewport.width.max(1.0), height.max(1.0)));
        let active_thread_id = agents.active_thread_id.borrow().clone();
        let enabled = agents.state.get() == NativeAgentState::Ready;
        if total_rows == 0 {
            let query = agents.history_query.borrow();
            let message = if query.trim().is_empty() {
                if agents.history_archived.get() {
                    "No archived Codex chats.".to_owned()
                } else {
                    "No recent Codex chats yet.".to_owned()
                }
            } else {
                format!("No chats match “{}”.", query.trim())
            };
            let empty =
                NSTextField::wrappingLabelWithString(&NSString::from_str(&message), self.mtm());
            empty.setFrame(NSRect::new(
                NSPoint::new(16.0, height / 2.0 - 22.0),
                NSSize::new((viewport.width - 32.0).max(1.0), 44.0),
            ));
            empty.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewMinYMargin
                    | NSAutoresizingMaskOptions::ViewMaxYMargin,
            );
            empty.setAlignment(NSTextAlignment::Center);
            empty.setTextColor(Some(&NSColor::tertiaryLabelColor()));
            empty.setMaximumNumberOfLines(2);
            agents.threads_document.addSubview(&empty);
            return;
        }
        let mut row_top = height;
        for (id, title, provider, running, usage) in &agent_sessions {
            let selected = agent_surface_visible && active_agent_id == Some(*id);
            let usage_label = usage.map(AgentResourceUsage::sidebar_label);
            let state = if *running { "Running" } else { "Exited" };
            let provider = provider.as_deref().unwrap_or("Terminal agent");
            let resource = if *running {
                usage_label.as_deref().unwrap_or("Measuring resources…")
            } else {
                "Session ended"
            };
            let metadata = format!("{provider} · {state}");
            let visible_title = format!("{title}\n{metadata}\n{resource}");
            row_top -= AGENT_TERMINAL_ROW_HEIGHT;
            let card_y = row_top + AGENT_SIDEBAR_ROW_GAP / 2.0;
            let card_width = (viewport.width - 8.0).max(1.0);
            let card_height = AGENT_TERMINAL_ROW_HEIGHT - AGENT_SIDEBAR_ROW_GAP;
            let card = NSBox::initWithFrame(
                NSBox::alloc(self.mtm()),
                NSRect::new(
                    NSPoint::new(4.0, card_y),
                    NSSize::new(card_width, card_height),
                ),
            );
            card.setBoxType(NSBoxType::Custom);
            card.setTitlePosition(NSTitlePosition::NoTitle);
            card.setBorderWidth(1.0);
            card.setCornerRadius(9.0);
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
            card.setBorderColor(&border_color);
            card.setFillColor(&fill_color);
            card.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            card.setAccessibilityElement(false);

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
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("terminal"),
                Some(&NSString::from_str("Terminal agent session")),
            )
            .expect("macOS provides the terminal SF Symbol");
            let icon = NSImageView::imageViewWithImage(&image, self.mtm());
            icon.setFrame(NSRect::new(
                NSPoint::new(12.0, (card_height - 20.0) / 2.0),
                NSSize::new(20.0, 20.0),
            ));
            icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            icon.setContentTintColor(Some(&secondary_color));
            icon.setAccessibilityElement(false);
            card.addSubview(&icon);
            let label_width = (card_width - 88.0).max(1.0);
            let mut labels = Vec::with_capacity(3);
            for (text, y, font, color) in [
                (
                    title.as_str(),
                    card_height - 22.0,
                    NSFont::boldSystemFontOfSize(13.5),
                    primary_color.clone(),
                ),
                (
                    metadata.as_str(),
                    card_height - 40.0,
                    NSFont::systemFontOfSize(11.0),
                    secondary_color.clone(),
                ),
                (
                    resource,
                    8.0,
                    NSFont::systemFontOfSize(10.5),
                    secondary_color.clone(),
                ),
            ] {
                let label = NSTextField::labelWithString(&NSString::from_str(text), self.mtm());
                label.setFrame(NSRect::new(
                    NSPoint::new(40.0, y),
                    NSSize::new(label_width, 16.0),
                ));
                label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                label.setFont(Some(&font));
                label.setTextColor(Some(&color));
                label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                label.setAccessibilityElement(false);
                card.addSubview(&label);
                labels.push(label);
            }
            let [title_label, metadata_label, resource_label] = labels
                .try_into()
                .expect("terminal card creates three labels");
            agents.threads_document.addSubview(&card);

            // The clear control overlays the card so the whole card selects the session while
            // the close button remains an independent target, matching the GTK navigation row.
            let row = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::new(),
                    Some(self),
                    Some(sel!(selectTerminalSession:)),
                    self.mtm(),
                )
            };
            row.setTag(*id);
            row.setFrame(NSRect::new(
                NSPoint::new(4.0, card_y),
                NSSize::new(
                    (viewport.width - 48.0).max(1.0),
                    AGENT_TERMINAL_ROW_HEIGHT - AGENT_SIDEBAR_ROW_GAP,
                ),
            ));
            row.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            row.setButtonType(NSButtonType::PushOnPushOff);
            row.setState(if selected {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            row.setBordered(false);
            row.setAccessibilityLabel(Some(&NSString::from_str(&visible_title)));
            row.setToolTip(Some(&NSString::from_str(&format!(
                "{title} terminal agent session"
            ))));
            agents.threads_document.addSubview(&row);

            let close = unsafe {
                NSButton::buttonWithImage_target_action(
                    &NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str("xmark"),
                        Some(&NSString::from_str("Close agent session")),
                    )
                    .expect("macOS provides the close agent session SF Symbol"),
                    Some(self),
                    Some(sel!(closeTerminalSession:)),
                    self.mtm(),
                )
            };
            close.setTag(*id);
            close.setFrame(NSRect::new(
                NSPoint::new(
                    (viewport.width - 38.0).max(0.0),
                    card_y + (card_height - 30.0) / 2.0,
                ),
                NSSize::new(30.0, 30.0),
            ));
            close.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            close.setBezelStyle(NSBezelStyle::AccessoryBarAction);
            close.setToolTip(Some(&NSString::from_str("Close agent session")));
            agents.threads_document.addSubview(&close);
            agents.terminal_cards.borrow_mut().insert(
                *id,
                NativeAgentTerminalCard {
                    container: card,
                    icon,
                    selector: row,
                    title: title_label,
                    metadata: metadata_label,
                    resource: resource_label,
                },
            );
        }
        for (index, (thread, preview)) in threads.iter().zip(thread_previews).enumerate() {
            let selected =
                !agent_surface_visible && active_thread_id.as_deref() == Some(thread.id.as_str());
            let mut metadata = Vec::new();
            if let Some(model) = thread.model.as_deref() {
                metadata.push(model);
            }
            if let Some(status) = thread.status.as_deref() {
                metadata.push(status);
            }
            if thread.archived {
                metadata.push("Archived");
            }
            let metadata = if metadata.is_empty() {
                "Codex".to_owned()
            } else {
                metadata.join("  •  ")
            };
            let time = craic_agent::display::relative_time(thread.updated_at);
            let visible_title = if preview.is_empty() {
                format!("{}\n{time}\n{metadata}", thread.title)
            } else {
                format!("{}\n{time}\n{preview}\n{metadata}", thread.title)
            };
            let row_height = if preview.is_empty() {
                AGENT_THREAD_COMPACT_ROW_HEIGHT
            } else {
                AGENT_THREAD_PREVIEW_ROW_HEIGHT
            };
            row_top -= row_height;
            let card_y = row_top + AGENT_SIDEBAR_ROW_GAP / 2.0;
            let card_width = (viewport.width - 8.0).max(1.0);
            let card_height = row_height - AGENT_SIDEBAR_ROW_GAP;
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
            let card = NSBox::initWithFrame(
                NSBox::alloc(self.mtm()),
                NSRect::new(
                    NSPoint::new(4.0, card_y),
                    NSSize::new(card_width, card_height),
                ),
            );
            card.setBoxType(NSBoxType::Custom);
            card.setTitlePosition(NSTitlePosition::NoTitle);
            card.setBorderWidth(1.0);
            card.setCornerRadius(9.0);
            let fill_color = if selected {
                NSColor::selectedContentBackgroundColor()
            } else {
                NSColor::controlBackgroundColor()
            };
            let border_color = if selected {
                NSColor::controlAccentColor()
            } else {
                NSColor::separatorColor()
            };
            card.setBorderColor(&border_color);
            card.setFillColor(&fill_color);
            card.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            card.setAccessibilityElement(false);

            let mut title_x = 12.0;
            if let Some((symbol, description)) = thread
                .pinned
                .then_some(("pin.fill", "Pinned Codex chat"))
                .or_else(|| {
                    thread
                        .archived
                        .then_some(("archivebox", "Archived Codex chat"))
                })
                && let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(description)),
                )
            {
                let icon = NSImageView::imageViewWithImage(&image, self.mtm());
                icon.setFrame(NSRect::new(
                    NSPoint::new(12.0, card_height - 25.0),
                    NSSize::new(16.0, 16.0),
                ));
                icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                icon.setContentTintColor(Some(&secondary_color));
                icon.setAccessibilityElement(false);
                card.addSubview(&icon);
                title_x = 34.0;
            }
            let time_width = 66.0;
            let time_x = (card_width - time_width - 12.0).max(title_x);
            let title_width = (time_x - title_x - 8.0).max(1.0);
            let heading_y = card_height - 25.0;
            let title =
                NSTextField::labelWithString(&NSString::from_str(&thread.title), self.mtm());
            title.setFrame(NSRect::new(
                NSPoint::new(title_x, heading_y),
                NSSize::new(title_width, 18.0),
            ));
            title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            title.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            title.setTextColor(Some(&primary_color));
            title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            title.setAccessibilityElement(false);
            card.addSubview(&title);

            let time_label = NSTextField::labelWithString(&NSString::from_str(&time), self.mtm());
            time_label.setFrame(NSRect::new(
                NSPoint::new(time_x, heading_y),
                NSSize::new(time_width, 17.0),
            ));
            time_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            time_label.setAlignment(NSTextAlignment::Right);
            time_label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            time_label.setTextColor(Some(&secondary_color));
            time_label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            time_label.setAccessibilityElement(false);
            card.addSubview(&time_label);

            if !preview.is_empty() {
                let preview_label =
                    NSTextField::wrappingLabelWithString(&NSString::from_str(preview), self.mtm());
                preview_label.setFrame(NSRect::new(
                    NSPoint::new(12.0, 27.0),
                    NSSize::new((card_width - 24.0).max(1.0), 30.0),
                ));
                preview_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                preview_label.setFont(Some(&NSFont::systemFontOfSize(11.5)));
                preview_label.setTextColor(Some(&secondary_color));
                preview_label.setMaximumNumberOfLines(2);
                preview_label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                preview_label.setAccessibilityElement(false);
                card.addSubview(&preview_label);
            }

            let metadata_label =
                NSTextField::labelWithString(&NSString::from_str(&metadata), self.mtm());
            metadata_label.setFrame(NSRect::new(
                NSPoint::new(12.0, 7.0),
                NSSize::new((card_width - 24.0).max(1.0), 15.0),
            ));
            metadata_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            metadata_label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            metadata_label.setTextColor(Some(&secondary_color));
            metadata_label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            metadata_label.setAccessibilityElement(false);
            card.addSubview(&metadata_label);
            agents.threads_document.addSubview(&card);

            // Keep activation and the context menu on one transparent whole-row target while
            // the source-list card owns the selected background and two-line typography.
            let row = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::new(),
                    Some(self),
                    Some(sel!(resumeAgentThread:)),
                    self.mtm(),
                )
            };
            row.setTag(index as isize);
            row.setFrame(NSRect::new(
                NSPoint::new(4.0, card_y),
                NSSize::new(card_width, card_height),
            ));
            row.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            row.setButtonType(NSButtonType::PushOnPushOff);
            row.setState(if selected {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            row.setBordered(false);
            row.setAccessibilityLabel(Some(&NSString::from_str(&visible_title)));
            row.setEnabled(enabled);
            let detail = if preview.is_empty() {
                metadata
            } else if metadata.is_empty() {
                preview.to_owned()
            } else {
                format!("{metadata}\n{preview}")
            };
            if !detail.is_empty() {
                row.setToolTip(Some(&NSString::from_str(&detail)));
            }
            let menu = NSMenu::new(self.mtm());
            for (title, action, symbol) in [("Rename…", sel!(renameAgentThread:), "pencil")] {
                let item = unsafe {
                    menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::new(),
                    )
                };
                item.setTag(index as isize);
                item.setEnabled(enabled);
                unsafe { item.setTarget(Some(self)) };
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(title)),
                ) {
                    item.setImage(Some(&image));
                }
            }
            let (archive_title, archive_action, archive_symbol) = if thread.archived {
                (
                    "Restore",
                    sel!(unarchiveAgentThread:),
                    "arrow.uturn.backward",
                )
            } else {
                ("Archive", sel!(archiveAgentThread:), "archivebox")
            };
            let archive = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(archive_title),
                    Some(archive_action),
                    &NSString::new(),
                )
            };
            archive.setTag(index as isize);
            archive.setEnabled(enabled);
            unsafe { archive.setTarget(Some(self)) };
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(archive_symbol),
                Some(&NSString::from_str(archive_title)),
            ) {
                archive.setImage(Some(&image));
            }
            menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            let delete = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str("Delete…"),
                    Some(sel!(deleteAgentThread:)),
                    &NSString::new(),
                )
            };
            delete.setTag(index as isize);
            delete.setEnabled(enabled);
            unsafe { delete.setTarget(Some(self)) };
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("trash"),
                Some(&NSString::from_str("Delete Codex thread")),
            ) {
                delete.setImage(Some(&image));
            }
            // SAFETY: The row, menu, and delegate target are retained main-thread objects.
            unsafe { row.setMenu(Some(&menu)) };
            agents.threads_document.addSubview(&row);
        }
        if keep_scrolled_to_top {
            threads_clip.scrollToPoint(NSPoint::new(0.0, (height - viewport.height).max(0.0)));
            agents.threads_scroll.reflectScrolledClipView(&threads_clip);
        }
    }

    fn make_native_agent_transcript_cell(
        &self,
        table: &NSTableView,
        row: usize,
    ) -> Option<Retained<NSView>> {
        let agents = self.ivars().agents.get()?;
        let item = agents.transcript_items.borrow().get(row)?.clone();
        let width = table.bounds().size.width.max(320.0);
        let agent_font_size = self.ivars().font_sizes.get().agent;
        let row_height = native_agent_transcript_row_height(&item, width, agent_font_size);
        let card_width = (width - 24.0).max(1.0);
        let card_height = (row_height - 12.0).max(1.0);
        let cell = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(width, row_height)),
        );
        cell.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let card = NSBox::initWithFrame(
            NSBox::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(12.0, 6.0),
                NSSize::new(card_width, card_height),
            ),
        );
        card.setBoxType(NSBoxType::Custom);
        card.setTitlePosition(NSTitlePosition::NoTitle);
        card.setBorderWidth(1.0);
        card.setCornerRadius(10.0);
        card.setBorderColor(&NSColor::separatorColor());
        card.setFillColor(&NSColor::controlBackgroundColor());
        card.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

        let (default_title, symbol) = match item.kind {
            NativeAgentTranscriptKind::User => ("You", "person.crop.circle.fill"),
            NativeAgentTranscriptKind::Assistant => ("Codex", "sparkles"),
            NativeAgentTranscriptKind::Developer => ("Developer message", "info.circle"),
            NativeAgentTranscriptKind::Reasoning => ("Reasoning", "brain.head.profile"),
            NativeAgentTranscriptKind::Plan => ("Plan", "list.bullet.clipboard"),
            NativeAgentTranscriptKind::Command => ("Command", "terminal"),
            NativeAgentTranscriptKind::FileChange => ("File changes", "doc.badge.gearshape"),
            NativeAgentTranscriptKind::Tool => ("Tool", "wrench.and.screwdriver"),
            NativeAgentTranscriptKind::McpTool => ("MCP tool", "network"),
            NativeAgentTranscriptKind::Web => ("Web", "globe"),
            NativeAgentTranscriptKind::Image => ("Image", "photo"),
            NativeAgentTranscriptKind::Collaboration => ("Collaboration", "person.2"),
            NativeAgentTranscriptKind::Review => ("Review", "checkmark.seal"),
            NativeAgentTranscriptKind::Compaction => ("Context compacted", "shippingbox"),
            NativeAgentTranscriptKind::Warning => ("Warning", "exclamationmark.triangle"),
            NativeAgentTranscriptKind::Error => ("Error", "exclamationmark.triangle.fill"),
        };
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(default_title)),
        ) {
            let icon = NSImageView::imageViewWithImage(&image, self.mtm());
            icon.setFrame(NSRect::new(
                NSPoint::new(14.0, card_height - 31.0),
                NSSize::new(17.0, 17.0),
            ));
            icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            let tint = match item.kind {
                NativeAgentTranscriptKind::Error => NSColor::systemRedColor(),
                NativeAgentTranscriptKind::Warning => NSColor::systemOrangeColor(),
                _ => NSColor::secondaryLabelColor(),
            };
            icon.setContentTintColor(Some(&tint));
            card.addSubview(&icon);
        }

        let title = item.title.as_deref().unwrap_or(default_title);
        let heading = NSTextField::labelWithString(&NSString::from_str(title), self.mtm());
        heading.setFrame(NSRect::new(
            NSPoint::new(39.0, card_height - 33.0),
            NSSize::new(card_width - 79.0, 20.0),
        ));
        heading.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        let title_color = match item.kind {
            NativeAgentTranscriptKind::Error => NSColor::systemRedColor(),
            NativeAgentTranscriptKind::Warning => NSColor::systemOrangeColor(),
            _ => NSColor::labelColor(),
        };
        heading.setTextColor(Some(&title_color));
        heading.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        card.addSubview(&heading);

        match item.status {
            NativeAgentTranscriptStatus::Running => {
                let status = NSProgressIndicator::initWithFrame(
                    NSProgressIndicator::alloc(self.mtm()),
                    NSRect::new(
                        NSPoint::new(card_width - 31.0, card_height - 31.0),
                        NSSize::new(16.0, 16.0),
                    ),
                );
                status.setStyle(NSProgressIndicatorStyle::Spinning);
                status.setControlSize(NSControlSize::Small);
                status.setDisplayedWhenStopped(false);
                status.setToolTip(Some(&NSString::from_str("Running")));
                status.setAccessibilityLabel(Some(&NSString::from_str("Running")));
                status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
                unsafe { status.startAnimation(None) };
                card.addSubview(&status);
            }
            status => {
                let (symbol, label, tint) = match status {
                    NativeAgentTranscriptStatus::Completed => (
                        "checkmark.circle.fill",
                        "Completed",
                        NSColor::secondaryLabelColor(),
                    ),
                    NativeAgentTranscriptStatus::Failed => {
                        ("xmark.circle.fill", "Failed", NSColor::systemRedColor())
                    }
                    NativeAgentTranscriptStatus::Interrupted => {
                        ("stop.circle", "Interrupted", NSColor::secondaryLabelColor())
                    }
                    NativeAgentTranscriptStatus::Running => unreachable!(),
                };
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(label)),
                ) {
                    let status = NSImageView::imageViewWithImage(&image, self.mtm());
                    status.setFrame(NSRect::new(
                        NSPoint::new(card_width - 31.0, card_height - 31.0),
                        NSSize::new(16.0, 16.0),
                    ));
                    status.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                    status.setContentTintColor(Some(&tint));
                    status.setToolTip(Some(&NSString::from_str(label)));
                    status.setAccessibilityLabel(Some(&NSString::from_str(label)));
                    status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
                    card.addSubview(&status);
                }
            }
        }

        let content_width = (card_width - 28.0).max(1.0);
        let mut cursor = card_height - 42.0;
        if !item.body.trim().is_empty() {
            let body_height = native_agent_transcript_section_height(
                item.body.trim_end(),
                content_width,
                agent_font_size,
                if native_agent_transcript_is_compact(item.kind) {
                    180.0
                } else {
                    300.0
                },
            );
            cursor -= body_height;
            let body = self.make_native_agent_transcript_text(
                item.body.trim_end(),
                NSRect::new(
                    NSPoint::new(14.0, cursor),
                    NSSize::new(content_width, body_height),
                ),
                false,
            );
            card.addSubview(&body);
            cursor -= 8.0;
        }
        if let Some(source) = item.image.as_ref() {
            let image_height = 200.0;
            cursor -= image_height;
            if let Some(image) = agents
                .transcript_images
                .borrow()
                .get(&item.id)
                .filter(|cached| &cached.source == source)
                .map(|cached| cached.image.clone())
            {
                let preview = NSImageView::imageViewWithImage(&image, self.mtm());
                preview.setFrame(NSRect::new(
                    NSPoint::new(14.0, cursor),
                    NSSize::new(content_width, image_height),
                ));
                preview.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                preview.setAccessibilityLabel(Some(&NSString::from_str(
                    item.title.as_deref().unwrap_or("Codex image"),
                )));
                card.addSubview(&preview);
            } else if let Some(message) = agents
                .transcript_image_errors
                .borrow()
                .get(&item.id)
                .filter(|failed| &failed.source == source)
                .map(|failed| failed.message.clone())
            {
                let unavailable = NSTextField::wrappingLabelWithString(
                    &NSString::from_str(&format!("Image preview unavailable\n{message}")),
                    self.mtm(),
                );
                unavailable.setFrame(NSRect::new(
                    NSPoint::new(14.0, cursor + 70.0),
                    NSSize::new(content_width, 60.0),
                ));
                unavailable.setAlignment(NSTextAlignment::Center);
                unavailable.setTextColor(Some(&NSColor::secondaryLabelColor()));
                card.addSubview(&unavailable);
            } else {
                let loading = NSProgressIndicator::initWithFrame(
                    NSProgressIndicator::alloc(self.mtm()),
                    NSRect::new(
                        NSPoint::new((card_width - 24.0) / 2.0, cursor + 88.0),
                        NSSize::new(24.0, 24.0),
                    ),
                );
                loading.setStyle(NSProgressIndicatorStyle::Spinning);
                loading.setDisplayedWhenStopped(false);
                loading.setAccessibilityLabel(Some(&NSString::from_str("Loading image")));
                unsafe { loading.startAnimation(None) };
                card.addSubview(&loading);
            }
            cursor -= 8.0;
        }
        if let Some(detail) = item
            .detail
            .as_deref()
            .filter(|detail| !detail.trim().is_empty())
        {
            let label = NSTextField::labelWithString(&NSString::from_str("Details"), self.mtm());
            cursor -= 16.0;
            label.setFrame(NSRect::new(
                NSPoint::new(14.0, cursor),
                NSSize::new(content_width, 16.0),
            ));
            label.setFont(Some(&NSFont::boldSystemFontOfSize(10.5)));
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            card.addSubview(&label);
            let detail_height = native_agent_transcript_section_height(
                detail.trim_end(),
                content_width,
                agent_font_size,
                220.0,
            );
            cursor -= detail_height + 4.0;
            let detail = self.make_native_agent_transcript_text(
                detail.trim_end(),
                NSRect::new(
                    NSPoint::new(14.0, cursor),
                    NSSize::new(content_width, detail_height),
                ),
                true,
            );
            card.addSubview(&detail);
        }

        cell.addSubview(&card);
        Some(cell)
    }

    fn make_native_agent_transcript_text(
        &self,
        text: &str,
        frame: NSRect,
        monospace: bool,
    ) -> Retained<NSScrollView> {
        let font_size = self.ivars().font_sizes.get().agent;
        let natural_height =
            native_agent_transcript_natural_text_height(text, frame.size.width, font_size);
        let text_view = NSTextView::initWithFrame(
            NSTextView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(frame.size.width, natural_height.max(frame.size.height)),
            ),
        );
        text_view.setEditable(false);
        text_view.setSelectable(true);
        text_view.setRichText(false);
        text_view.setDrawsBackground(false);
        text_view.setTextContainerInset(NSSize::new(2.0, 2.0));
        text_view.setDelegate(Some(ProtocolObject::from_ref(self)));
        let attributed = native_agent_attributed_text(text, monospace, font_size);
        if let Some(storage) = unsafe { text_view.textStorage() } {
            storage.setAttributedString(&attributed);
        }

        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(self.mtm()), frame);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setAutomaticallyAdjustsContentInsets(false);
        scroll.setHasVerticalScroller(natural_height > frame.size.height);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&text_view));
        scroll
    }

    fn render_native_agent_transcript(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let count = agents.transcript_items.borrow().len();
        agents.transcript_table.reloadData();
        agents.empty.setHidden(count != 0);
        agents.transcript_scroll.setHidden(count == 0);
        if count != 0 {
            agents
                .transcript_table
                .scrollRowToVisible(count as isize - 1);
        }
    }

    fn request_native_agent_transcript_image(
        &self,
        identity: &AgentIdentity,
        item: &NativeAgentTranscriptItem,
    ) {
        let Some(source) = item.image.clone() else {
            return;
        };
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if agents
            .transcript_images
            .borrow()
            .get(&item.id)
            .is_some_and(|cached| cached.source == source)
            || agents
                .transcript_image_in_flight
                .borrow()
                .get(&item.id)
                .is_some_and(|pending| pending == &source)
            || agents
                .transcript_image_errors
                .borrow()
                .get(&item.id)
                .is_some_and(|failed| failed.source == source)
        {
            return;
        }
        let Some(access) = self
            .ivars()
            .git_handle
            .borrow()
            .as_ref()
            .map(|handle| handle.workspace_files())
        else {
            return;
        };
        let (Some(requests), Some(cancellation)) = (
            self.ivars().repository_requests.get(),
            self.workspace_cancellation_token(),
        ) else {
            return;
        };
        agents
            .transcript_image_in_flight
            .borrow_mut()
            .insert(item.id.clone(), source.clone());
        agents.transcript_image_errors.borrow_mut().remove(&item.id);
        if let Err(error) = requests.try_send(RepositoryRequest::LoadAgentImage {
            workspace_id: identity.workspace_id.clone(),
            generation: identity.generation,
            item_id: item.id.clone(),
            source: source.clone(),
            access,
            cancellation,
        }) {
            agents
                .transcript_image_in_flight
                .borrow_mut()
                .remove(&item.id);
            agents.transcript_image_errors.borrow_mut().insert(
                item.id.clone(),
                NativeAgentTranscriptImageError {
                    source,
                    message: format!("Unable to queue image load: {error}"),
                },
            );
        }
    }

    fn apply_native_agent_transcript_image(
        &self,
        workspace_id: &str,
        generation: u64,
        item_id: &str,
        source: NativeAgentTranscriptImageSource,
        result: Result<Vec<u8>, String>,
    ) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || agents.generation.get() != generation
            || !agents
                .transcript_items
                .borrow()
                .iter()
                .any(|item| item.id == item_id && item.image.as_ref() == Some(&source))
        {
            return;
        }
        if agents.transcript_image_in_flight.borrow().get(item_id) != Some(&source) {
            return;
        }
        agents
            .transcript_image_in_flight
            .borrow_mut()
            .remove(item_id);
        let image = result.and_then(|bytes| {
            NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&bytes))
                .ok_or_else(|| "macOS could not decode this image.".to_string())
        });
        match image {
            Ok(image) => {
                agents.transcript_image_errors.borrow_mut().remove(item_id);
                agents.transcript_images.borrow_mut().insert(
                    item_id.to_string(),
                    NativeAgentTranscriptImage { source, image },
                );
                let mut order = agents.transcript_image_order.borrow_mut();
                if let Some(index) = order.iter().position(|candidate| candidate == item_id) {
                    order.remove(index);
                }
                order.push_back(item_id.to_string());
                while order.len() > AGENT_IMAGE_CACHE_CAPACITY {
                    if let Some(evicted) = order.pop_front() {
                        agents.transcript_images.borrow_mut().remove(&evicted);
                    }
                }
                log::debug!("native Codex transcript image applied item_id={item_id}");
            }
            Err(message) => {
                log::warn!(
                    "native Codex transcript image unavailable item_id={item_id}: {message}"
                );
                agents.transcript_image_errors.borrow_mut().insert(
                    item_id.to_string(),
                    NativeAgentTranscriptImageError { source, message },
                );
            }
        }
        if let Some(row) = agents
            .transcript_items
            .borrow()
            .iter()
            .position(|item| item.id == item_id)
        {
            agents
                .transcript_table
                .noteHeightOfRowsWithIndexesChanged(&NSIndexSet::indexSetWithIndex(row));
            agents
                .transcript_table
                .reloadDataForRowIndexes_columnIndexes(
                    &NSIndexSet::indexSetWithIndex(row),
                    &NSIndexSet::indexSetWithIndex(0),
                );
        }
    }

    fn remove_native_agent_transcript_image(&self, item_id: &str) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents.transcript_images.borrow_mut().remove(item_id);
        agents
            .transcript_image_in_flight
            .borrow_mut()
            .remove(item_id);
        agents.transcript_image_errors.borrow_mut().remove(item_id);
        agents
            .transcript_image_order
            .borrow_mut()
            .retain(|candidate| candidate != item_id);
    }

    fn clear_native_agent_transcript_images(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents.transcript_images.borrow_mut().clear();
        agents.transcript_image_order.borrow_mut().clear();
        agents.transcript_image_in_flight.borrow_mut().clear();
        agents.transcript_image_errors.borrow_mut().clear();
    }

    fn reset_native_agent_ui(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        for (_, alert) in self.ivars().agent_request_alerts.borrow_mut().drain() {
            alert.window().close();
        }
        self.ivars()
            .agent_request_multiline_inputs
            .borrow_mut()
            .clear();
        self.ivars().agent_pending_request_keys.borrow_mut().clear();
        agents.state.set(NativeAgentState::Closed);
        agents
            .new_chat
            .setEnabled(self.ivars().active_workspace_id.borrow().is_some());
        agents.transcript_items.borrow_mut().clear();
        self.clear_native_agent_transcript_images();
        agents.transcript_table.reloadData();
        agents.composer.setString(&NSString::new());
        agents.attachments.borrow_mut().clear();
        self.refresh_native_agent_attachments();
        agents
            .title
            .setStringValue(&NSString::from_str("New Codex chat"));
        agents
            .status
            .setStringValue(&NSString::from_str("No active session"));
        unsafe { agents.spinner.stopAnimation(None) };
        agents.spinner.setHidden(true);
        agents.transcript_scroll.setHidden(true);
        agents.empty.setStringValue(&NSString::from_str(
            "Start a new Codex chat from the sidebar.",
        ));
        agents.empty.setHidden(false);
        agents.composer_scroll.setHidden(true);
        agents.attach.setHidden(true);
        agents.attachment_tokens.setHidden(true);
        agents.clear_attachments.setHidden(true);
        agents.send.setHidden(true);
        agents.stop.setHidden(true);
        agents.model.setEnabled(false);
        agents.reasoning.setEnabled(false);
        agents.personality.setEnabled(false);
        agents.service_tier.setEnabled(false);
        agents.permissions.setEnabled(false);
        self.apply_native_agent_usage(None);
        self.refresh_native_agent_thread_rows();
        self.set_page_badge("agents", NativePageBadge::None);
    }

}
