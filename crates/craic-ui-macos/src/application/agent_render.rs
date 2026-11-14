impl AppDelegate {
    fn reset_native_agent_ui(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents
            .history_request_id
            .set(agents.history_request_id.get().wrapping_add(1));
        agents.history_loading.set(false);
        agents.history_started_at.set(None);
        if let Some(cancellation) = agents.history_cancellation.borrow_mut().take() {
            cancellation.cancel();
        }
        agents.default_launch_attempted.set(false);
        agents.history_request_filter.replace(Default::default());
        agents.history_refresh_pending.set(false);
        agents.history_loaded.set(false);
        agents.history_last_refresh_ms.set(0);
        agents.history_has_more.set(false);
        agents.history_limit.set(32);
        agents.history_rows.borrow_mut().clear();
        agents.history_tags.borrow_mut().clear();
        agents.selected_history_tags.borrow_mut().clear();
        agents.threads.borrow_mut().clear();
        agents.history_query.borrow_mut().clear();
        agents.history_search.setStringValue(&NSString::new());
        agents
            .history_status
            .setStringValue(&NSString::from_str("Loading history…"));
        agents
            .new_chat
            .setEnabled(self.ivars().active_workspace_id.borrow().is_some());
        self.reset_native_provider_updates();
        self.refresh_native_agent_history_tags();
        self.refresh_native_agent_thread_rows();
        self.restore_agents_page_badge();
    }

    fn refresh_native_agent_thread_rows(&self) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let query = craic_agent::agent_history::normalize_title(&agents.history_query.borrow())
            .to_lowercase();
        let tag_filter_active = !agents.selected_history_tags.borrow().is_empty();
        let history_filter_active = tag_filter_active || !query.is_empty();
        let loaded_ids = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .filter(|session| session.placement == NativeTerminalPlacement::Agent)
            .filter_map(|session| session.agent_history_id)
            .collect::<HashSet<_>>();
        let history_threads = agents
            .history_rows
            .borrow()
            .iter()
            .filter(|row| history_filter_active || !loaded_ids.contains(&row.id))
            .map(|row| NativeAgentThreadSummary {
                id: row.id.to_string(),
                title: row.title.clone(),
                preview: String::new(),
                smart_summary: row.task_description.clone(),
                model: Some(
                    NATIVE_AGENT_PROVIDERS
                        .iter()
                        .find(|provider| provider.provider_id() == row.provider_id)
                        .map(|provider| provider.label())
                        .unwrap_or("Agent")
                        .to_owned(),
                ),
                status: Some(
                    if row.restore_state.is_restorable() {
                        "Unloaded"
                    } else {
                        "Restore unavailable"
                    }
                    .to_owned(),
                ),
                updated_at: row.last_seen_at_ms,
                pinned: false,
                archived: false,
            })
            .collect();
        agents.threads.replace(history_threads);
        let active_agent_id = self.ivars().active_agent_terminal_id.get();
        let agent_surface_visible = !agents.terminal_panel.isHidden();
        let usage = self.ivars().agent_terminal_usage.borrow();
        let agent_sessions = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .filter(|session| session.placement == NativeTerminalPlacement::Agent)
            .filter(|session| {
                if session.agent_history_id.is_some() && history_filter_active {
                    return false;
                }
                if session.agent_history_id.is_none() && tag_filter_active {
                    return false;
                }
                craic_agent::agent_history::normalize_title(&session.title_label)
                    .to_lowercase()
                    .contains(&query)
            })
            .map(|session| {
                (
                    session.id,
                    session.title_label.clone(),
                    session.agent_provider_label.clone(),
                    session.view.is_active(),
                    usage.get(&session.id).copied(),
                    session.agent_active_state,
                    session.agent_provider.and_then(|provider| {
                        agents.provider_icons.get(provider.provider_id()).cloned()
                    }),
                    session.agent_last_seen_ms,
                    session.agent_history_id.is_none(),
                    session.launch_cancellation.is_some(),
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
        let threads_clip = agents.threads_scroll.contentView();
        let viewport = threads_clip.bounds().size;
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
        let mut timeline = agent_sessions
            .iter()
            .map(|session| {
                (
                    session.8,
                    session.7,
                    session.0,
                    true,
                    AGENT_TERMINAL_ROW_HEIGHT,
                )
            })
            .collect::<Vec<_>>();
        timeline.extend(
            threads
                .iter()
                .zip(&thread_previews)
                .map(|(thread, preview)| {
                    (
                        false,
                        thread.updated_at,
                        thread.id.parse::<isize>().unwrap_or(-1),
                        false,
                        if preview.is_empty() {
                            AGENT_THREAD_COMPACT_ROW_HEIGHT
                        } else {
                            AGENT_THREAD_PREVIEW_ROW_HEIGHT
                        },
                    )
                }),
        );
        timeline.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.cmp(&left.1))
                .then_with(|| right.2.cmp(&left.2))
        });
        let month = NSDateFormatter::new();
        month.setDateFormat(Some(&NSString::from_str("MMMM yyyy")));
        let now = craic_agent::agent_history::unix_now_ms();
        let day = 24 * 60 * 60 * 1000_i64;
        let mut current_group = String::new();
        let mut layout = Vec::new();
        let mut content_height = 24.0;
        for (pinned, timestamp, id, active, row_height) in timeline {
            if !pinned {
                let age = now.saturating_sub(timestamp);
                let group = if age < day {
                    "Today".to_owned()
                } else if age < day * 2 {
                    "Yesterday".to_owned()
                } else if age < day * 7 {
                    "Last 7 Days".to_owned()
                } else if age < day * 30 {
                    "Last 30 Days".to_owned()
                } else {
                    month
                        .stringFromDate(&NSDate::dateWithTimeIntervalSince1970(
                            timestamp as f64 / 1000.0,
                        ))
                        .to_string()
                };
                if group != current_group {
                    current_group = group.clone();
                    layout.push((None, group, 26.0));
                    content_height += 26.0;
                }
            }
            layout.push((Some((active, id)), String::new(), row_height));
            content_height += row_height;
        }
        let height = content_height.max(viewport.height);
        agents
            .threads_document
            .setFrameSize(NSSize::new(viewport.width.max(1.0), height.max(1.0)));
        let active_history_id = active_agent_id.and_then(|id| {
            self.ivars()
                .terminal_sessions
                .borrow()
                .iter()
                .find(|session| session.id == id)
                .and_then(|session| session.agent_history_id)
        });
        let mut row_positions = HashMap::new();
        let mut row_top = height - 12.0;
        for (key, title, row_height) in layout {
            row_top -= row_height;
            if let Some(key) = key {
                row_positions.insert(key, row_top);
            } else {
                let label = NSTextField::labelWithString(&NSString::from_str(&title), self.mtm());
                label.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
                label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                label.setFrame(NSRect::new(
                    NSPoint::new(12.0, row_top + 4.0),
                    NSSize::new((viewport.width - 24.0).max(1.0), 18.0),
                ));
                label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                agents.threads_document.addSubview(&label);
            }
        }
        let enabled = self.ivars().workspace_handle.borrow().is_some();
        if total_rows == 0 {
            if agents.history_loading.get() && !agents.history_loaded.get() {
                return;
            }
            let query = agents.history_query.borrow();
            let message = if query.trim().is_empty() {
                "No recent agent sessions yet.".to_owned()
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
        for (id, title, provider, running, usage, active_state, provider_icon, _, _, preparing) in
            &agent_sessions
        {
            let selected = agent_surface_visible && active_agent_id == Some(*id);
            let usage_label = usage.map(AgentResourceUsage::sidebar_label);
            let state = if *preparing {
                "Starting"
            } else if *running {
                match active_state {
                    AgentActiveState::NewChat => "New chat",
                    AgentActiveState::Idle => "Idle",
                    AgentActiveState::Loading => "Working",
                    AgentActiveState::Asking => "Waiting for input",
                }
            } else {
                "Exited"
            };
            let provider = provider.as_deref().unwrap_or("Terminal agent");
            let resource = if *preparing {
                "Starting terminal…"
            } else if *running {
                usage_label.as_deref().unwrap_or("Measuring resources…")
            } else {
                "Session ended"
            };
            let metadata = format!("{provider} · {state}");
            let visible_title = format!("{title}\n{metadata}\n{resource}");
            let card_y = row_positions[&(true, *id)] + AGENT_SIDEBAR_ROW_GAP / 2.0;
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
            card.setContentViewMargins(NSSize::ZERO);
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
            let image = if *running && *active_state == AgentActiveState::Asking {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("hand.tap.fill"),
                    Some(&NSString::from_str("Waiting for input")),
                )
            } else {
                provider_icon.clone()
            }
            .or_else(|| {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("terminal"),
                    Some(&NSString::from_str("Terminal agent session")),
                )
            })
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
            let local_id = self
                .ivars()
                .terminal_sessions
                .borrow()
                .iter()
                .find(|session| session.id == *id)
                .and_then(|session| session.agent_history_id);
            unsafe {
                row.setMenu(Some(
                    &self.make_native_agent_session_menu(Some(*id), local_id),
                ));
            }
            agents.threads_document.addSubview(&row);

            let close_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("xmark"),
                Some(&NSString::from_str("Close agent session")),
            )
            .expect("macOS provides the close agent session SF Symbol");
            close_image.setSize(NSSize::new(10.0, 10.0));
            let close = unsafe {
                NSButton::buttonWithImage_target_action(
                    &close_image,
                    Some(self),
                    Some(sel!(closeTerminalSession:)),
                    self.mtm(),
                )
            };
            close.setTag(*id);
            close.setFrame(NSRect::new(
                NSPoint::new(
                    (viewport.width - 34.0).max(0.0),
                    card_y + (card_height - 22.0) / 2.0,
                ),
                NSSize::new(22.0, 22.0),
            ));
            close.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            close.setBordered(false);
            close.setButtonType(NSButtonType::MomentaryChange);
            close.setControlSize(NSControlSize::Small);
            close.setImageScaling(NSImageScaling::ScaleProportionallyDown);
            close.setContentTintColor(Some(&secondary_color));
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
        let time_formatter = NSDateFormatter::new();
        time_formatter.setDateFormat(Some(&NSString::from_str("MMM dd, hh:mm a")));
        for (thread, preview) in threads.iter().zip(thread_previews) {
            let selected =
                agent_surface_visible && active_history_id == thread.id.parse::<i64>().ok();
            let time = time_formatter
                .stringFromDate(&NSDate::dateWithTimeIntervalSince1970(
                    thread.updated_at as f64 / 1000.0,
                ))
                .to_string();
            let metadata = if let Some(status) = thread
                .status
                .as_deref()
                .filter(|status| *status != "Unloaded")
            {
                format!(
                    "{} · {time} · {status}",
                    thread.model.as_deref().unwrap_or("Agent")
                )
            } else {
                format!("{} · {time}", thread.model.as_deref().unwrap_or("Agent"))
            };
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
            let card_y = row_positions[&(false, thread.id.parse::<isize>().unwrap_or(-1))]
                + AGENT_SIDEBAR_ROW_GAP / 2.0;
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
            card.setContentViewMargins(NSSize::ZERO);
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
            if let Some(image) = thread.model.as_deref().and_then(|label| {
                NATIVE_AGENT_PROVIDERS
                    .iter()
                    .find(|provider| provider.label() == label)
                    .and_then(|provider| agents.provider_icons.get(provider.provider_id()))
            }) {
                let icon = NSImageView::imageViewWithImage(image, self.mtm());
                icon.setFrame(NSRect::new(
                    NSPoint::new(12.0, card_height - 25.0),
                    NSSize::new(18.0, 18.0),
                ));
                icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
                icon.setContentTintColor(Some(&secondary_color));
                icon.setAlphaValue(0.58);
                card.addSubview(&icon);
                title_x = 38.0;
            }
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
            let title_width = (card_width - title_x - 12.0).max(1.0);
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
            row.setTag(thread.id.parse::<isize>().unwrap_or(-1));
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
            let menu = self.make_native_agent_session_menu(None, thread.id.parse::<i64>().ok());
            // SAFETY: The row, menu, and delegate target are retained main-thread objects.
            unsafe { row.setMenu(Some(&menu)) };
            agents.threads_document.addSubview(&row);
        }
        if keep_scrolled_to_top {
            threads_clip.scrollToPoint(NSPoint::new(0.0, (height - viewport.height).max(0.0)));
        } else {
            let anchored_y = (previous_scroll_y + height - previous_document_height)
                .clamp(0.0, (height - viewport.height).max(0.0));
            threads_clip.scrollToPoint(NSPoint::new(0.0, anchored_y));
        }
        agents.threads_scroll.reflectScrolledClipView(&threads_clip);
    }
}
