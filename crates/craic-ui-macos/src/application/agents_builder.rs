impl AppDelegate {
    fn make_agents_ui(&self, sidebar_bounds: NSRect, content_bounds: NSRect) -> AgentsUi {
        let mtm = self.mtm();
        let sidebar_root = NSView::initWithFrame(NSView::alloc(mtm), sidebar_bounds);
        sidebar_root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        sidebar_root.setHidden(true);

        let new_chat = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("App"),
                Some(self),
                Some(sel!(newAgentChat:)),
                mtm,
            )
        };
        new_chat.setFrame(NSRect::new(
            NSPoint::new(14.0, 14.0),
            NSSize::new(68.0, 32.0),
        ));
        new_chat.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        new_chat.setBordered(false);
        new_chat.setControlSize(NSControlSize::Regular);
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("plus"),
            Some(&NSString::from_str("New App chat")),
        ) {
            new_chat.setImage(Some(&image));
            new_chat.setImagePosition(NSCellImagePosition::ImageLeading);
        }
        new_chat.setToolTip(Some(&NSString::from_str("New App chat")));
        sidebar_root.addSubview(&new_chat);

        let mut terminal_agent_buttons = Vec::with_capacity(2);
        for (x, width, label, tooltip, action) in [
            (
                90.0,
                104.0,
                "Codex CLI",
                "New Codex CLI chat",
                sel!(newCodexCli:),
            ),
            (202.0, 62.0, "AGY", "New AGY chat", sel!(newAgy:)),
        ] {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(label),
                    Some(self),
                    Some(action),
                    mtm,
                )
            };
            button.setFrame(NSRect::new(NSPoint::new(x, 14.0), NSSize::new(width, 32.0)));
            button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
            button.setBordered(false);
            button.setControlSize(NSControlSize::Regular);
            button.setToolTip(Some(&NSString::from_str(tooltip)));
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("plus"),
                Some(&NSString::from_str(tooltip)),
            ) {
                button.setImage(Some(&image));
                button.setImagePosition(NSCellImagePosition::ImageLeading);
            }
            sidebar_root.addSubview(&button);
            terminal_agent_buttons.push(button);
        }
        let [codex_cli, agy] = terminal_agent_buttons
            .try_into()
            .expect("two terminal agent launch buttons are created");

        let history_search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            NSRect::new(
                NSPoint::new(14.0, sidebar_bounds.size.height - 216.0),
                NSSize::new((sidebar_bounds.size.width - 112.0).max(1.0), 28.0),
            ),
        );
        history_search.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        history_search.setPlaceholderString(Some(&NSString::from_str("Search chats")));
        history_search.setToolTip(Some(&NSString::from_str("Search Codex chat history")));
        history_search.setControlSize(NSControlSize::Small);
        history_search.setSendsSearchStringImmediately(true);
        history_search.setEnabled(false);
        history_search.setHidden(true);
        unsafe {
            history_search.setTarget(Some(self));
            history_search.setAction(Some(sel!(filterAgentThreads:)));
        }
        sidebar_root.addSubview(&history_search);

        let history_scope = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(
                NSPoint::new(
                    sidebar_bounds.size.width - 92.0,
                    sidebar_bounds.size.height - 216.0,
                ),
                NSSize::new(78.0, 28.0),
            ),
            false,
        );
        history_scope.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        history_scope.addItemWithTitle(&NSString::from_str("Recent"));
        history_scope.addItemWithTitle(&NSString::from_str("Archived"));
        history_scope.setToolTip(Some(&NSString::from_str("Codex chat history scope")));
        history_scope.setControlSize(NSControlSize::Small);
        history_scope.setEnabled(false);
        history_scope.setHidden(true);
        unsafe {
            history_scope.setTarget(Some(self));
            history_scope.setAction(Some(sel!(selectAgentThreadScope:)));
        }
        sidebar_root.addSubview(&history_scope);

        let threads_document = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new((sidebar_bounds.size.width - 16.0).max(1.0), 1.0),
            ),
        );
        let threads_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(8.0, 8.0),
                NSSize::new(
                    (sidebar_bounds.size.width - 16.0).max(1.0),
                    (sidebar_bounds.size.height - 236.0).max(1.0),
                ),
            ),
        );
        threads_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        threads_scroll.setBorderType(NSBorderType::NoBorder);
        threads_scroll.setDrawsBackground(false);
        threads_scroll.setHasVerticalScroller(true);
        threads_scroll.setAutohidesScrollers(true);
        threads_scroll.setDocumentView(Some(&threads_document));
        sidebar_root.addSubview(&threads_scroll);

        let content_root = NSView::initWithFrame(NSView::alloc(mtm), content_bounds);
        content_root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content_root.setHidden(true);

        let title = NSTextField::labelWithString(&NSString::from_str("New Codex chat"), mtm);
        title.setFont(Some(&NSFont::boldSystemFontOfSize(18.0)));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        content_root.addSubview(&title);

        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(16.0, 16.0)),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setIndeterminate(true);
        spinner.setDisplayedWhenStopped(false);
        spinner.setHidden(true);
        content_root.addSubview(&spinner);

        let status = NSTextField::labelWithString(&NSString::from_str("No active session"), mtm);
        status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        content_root.addSubview(&status);

        let tools =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, true);
        tools.addItemWithTitle(&NSString::new());
        if let Some(item) = tools.lastItem()
            && let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("wrench.and.screwdriver"),
                Some(&NSString::from_str("Codex tools")),
            )
        {
            item.setImage(Some(&image));
        }
        if let Some(menu) = tools.menu() {
            for (title, symbol, action) in [
                ("Thread Goal…", "target", sel!(showAgentThreadGoal:)),
                (
                    "Run Shell Command…",
                    "terminal",
                    sel!(runAgentShellCommand:),
                ),
                (
                    "Background Terminals",
                    "rectangle.stack.badge.play",
                    sel!(showAgentBackgroundTerminals:),
                ),
                ("Skills", "graduationcap", sel!(showAgentSkills:)),
                ("MCP Servers", "server.rack", sel!(showAgentMcpServers:)),
                (
                    "Apps & Connectors",
                    "app.connected.to.app.below.fill",
                    sel!(showAgentApps:),
                ),
                ("Plugins", "puzzlepiece.extension", sel!(showAgentPlugins:)),
                (
                    "Experimental Features",
                    "flask",
                    sel!(showAgentExperimentalFeatures:),
                ),
                (
                    "Account & Usage",
                    "person.crop.circle",
                    sel!(showAgentAccountUsage:),
                ),
            ] {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::new(),
                    )
                };
                unsafe { item.setTarget(Some(self)) };
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(title)),
                ) {
                    item.setImage(Some(&image));
                }
                menu.addItem(&item);
            }
        }
        tools.setControlSize(NSControlSize::Small);
        tools.setBordered(false);
        tools.setToolTip(Some(&NSString::from_str("Codex tools")));
        tools.setEnabled(false);
        content_root.addSubview(&tools);

        let thread_actions =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, true);
        thread_actions.addItemWithTitle(&NSString::new());
        if let Some(item) = thread_actions.lastItem()
            && let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("ellipsis.circle"),
                Some(&NSString::from_str("Thread actions")),
            )
        {
            item.setImage(Some(&image));
        }
        if let Some(menu) = thread_actions.menu() {
            for (title, symbol, action) in [
                (
                    "Thread History",
                    "clock.arrow.circlepath",
                    sel!(showAgentThreadHistory:),
                ),
                (
                    "Fork Thread",
                    "arrow.triangle.branch",
                    sel!(forkActiveAgentThread:),
                ),
                (
                    "Archive Thread",
                    "archivebox",
                    sel!(archiveActiveAgentThread:),
                ),
                (
                    "Compact Context",
                    "shippingbox",
                    sel!(compactActiveAgentThread:),
                ),
                ("Start Review…", "checkmark.bubble", sel!(startAgentReview:)),
                (
                    "Roll Back Last Turn",
                    "arrow.uturn.backward",
                    sel!(rollbackActiveAgentThread:),
                ),
                (
                    "Open Changes / Diff",
                    "pencil.and.list.clipboard",
                    sel!(openAgentChanges:),
                ),
            ] {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::new(),
                    )
                };
                unsafe { item.setTarget(Some(self)) };
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(title)),
                ) {
                    item.setImage(Some(&image));
                }
                menu.addItem(&item);
            }
        }
        thread_actions.setControlSize(NSControlSize::Small);
        thread_actions.setBordered(false);
        thread_actions.setToolTip(Some(&NSString::from_str("Thread actions")));
        thread_actions.setEnabled(false);
        content_root.addSubview(&thread_actions);

        let usage = NSTextField::labelWithString(&NSString::from_str("Context unavailable"), mtm);
        usage.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        usage.setTextColor(Some(&NSColor::secondaryLabelColor()));
        usage.setAlignment(NSTextAlignment::Right);
        usage.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        usage.setToolTip(Some(&NSString::from_str(
            "Token usage is not available yet",
        )));
        content_root.addSubview(&usage);

        let usage_progress = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(84.0, 8.0)),
        );
        usage_progress.setStyle(NSProgressIndicatorStyle::Bar);
        usage_progress.setIndeterminate(false);
        usage_progress.setMinValue(0.0);
        usage_progress.setMaxValue(1.0);
        usage_progress.setDoubleValue(0.0);
        usage_progress.setControlSize(NSControlSize::Small);
        usage_progress.setToolTip(Some(&NSString::from_str(
            "Token usage is not available yet",
        )));
        content_root.addSubview(&usage_progress);

        let selected_model = load_native_string_default(AGENT_MODEL_DEFAULT);
        let model =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        model.addItemWithTitle(&NSString::from_str(
            selected_model
                .as_deref()
                .unwrap_or("Start a chat to load models"),
        ));
        model.setToolTip(Some(&NSString::from_str("Codex model")));
        model.setControlSize(NSControlSize::Small);
        model.setEnabled(false);
        unsafe {
            model.setTarget(Some(self));
            model.setAction(Some(sel!(selectAgentModel:)));
        }
        content_root.addSubview(&model);

        let selected_reasoning = load_native_string_default(AGENT_REASONING_DEFAULT);
        let reasoning =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        reasoning.addItemWithTitle(&NSString::from_str(
            selected_reasoning.as_deref().unwrap_or("Reasoning"),
        ));
        reasoning.setToolTip(Some(&NSString::from_str("Codex reasoning effort")));
        reasoning.setControlSize(NSControlSize::Small);
        reasoning.setEnabled(false);
        unsafe {
            reasoning.setTarget(Some(self));
            reasoning.setAction(Some(sel!(selectAgentReasoning:)));
        }
        content_root.addSubview(&reasoning);

        let selected_personality = load_native_string_default(AGENT_PERSONALITY_DEFAULT);
        let personality =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        personality.addItemWithTitle(&NSString::from_str(
            selected_personality.as_deref().unwrap_or("Personality"),
        ));
        personality.setToolTip(Some(&NSString::from_str("Codex personality")));
        personality.setControlSize(NSControlSize::Small);
        personality.setEnabled(false);
        unsafe {
            personality.setTarget(Some(self));
            personality.setAction(Some(sel!(selectAgentPersonality:)));
        }
        content_root.addSubview(&personality);

        let selected_service_tier = load_native_string_default(AGENT_SERVICE_TIER_DEFAULT);
        let service_tier =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        service_tier.addItemWithTitle(&NSString::from_str(
            if selected_service_tier.is_none()
                || selected_service_tier.as_deref() == Some(DEFAULT_SERVICE_TIER_ID)
            {
                "Standard"
            } else {
                selected_service_tier.as_deref().unwrap_or("Response speed")
            },
        ));
        service_tier.setToolTip(Some(&NSString::from_str("Codex response speed")));
        service_tier.setControlSize(NSControlSize::Small);
        service_tier.setEnabled(false);
        unsafe {
            service_tier.setTarget(Some(self));
            service_tier.setAction(Some(sel!(selectAgentServiceTier:)));
        }
        content_root.addSubview(&service_tier);

        let selected_permissions = load_native_string_default(AGENT_PERMISSIONS_DEFAULT);
        let permissions =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        permissions.addItemWithTitle(&NSString::from_str(
            selected_permissions
                .as_deref()
                .map(permission_profile_label)
                .as_deref()
                .unwrap_or("Start a chat to load permissions"),
        ));
        permissions.setToolTip(Some(&NSString::from_str("Codex permission profile")));
        permissions.setControlSize(NSControlSize::Small);
        permissions.setEnabled(false);
        unsafe {
            permissions.setTarget(Some(self));
            permissions.setAction(Some(sel!(selectAgentPermissions:)));
        }
        content_root.addSubview(&permissions);

        let transcript_table = NSTableView::initWithFrame(
            NSTableView::alloc(mtm),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(content_bounds.size.width - 40.0, 1.0),
            ),
        );
        let transcript_column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(mtm),
            &NSUserInterfaceItemIdentifier::from_str("agent.transcript"),
        );
        transcript_column.setWidth(content_bounds.size.width - 40.0);
        transcript_table.addTableColumn(&transcript_column);
        transcript_table.setHeaderView(None);
        transcript_table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        transcript_table.setIntercellSpacing(NSSize::new(0.0, 0.0));
        transcript_table.setUsesAlternatingRowBackgroundColors(false);
        transcript_table.setAllowsEmptySelection(true);
        transcript_table.setAllowsMultipleSelection(false);
        transcript_table.setBackgroundColor(&NSColor::clearColor());
        unsafe {
            transcript_table.setDataSource(Some(ProtocolObject::from_ref(self)));
            transcript_table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let transcript_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), NSRect::ZERO);
        transcript_scroll.setBorderType(NSBorderType::NoBorder);
        transcript_scroll.setDrawsBackground(false);
        transcript_scroll.setHasVerticalScroller(true);
        transcript_scroll.setAutohidesScrollers(true);
        transcript_scroll.setDocumentView(Some(&transcript_table));
        content_root.addSubview(&transcript_scroll);

        let empty = NSTextField::wrappingLabelWithString(
            &NSString::from_str("Start a new Codex chat from the sidebar."),
            mtm,
        );
        empty.setAlignment(NSTextAlignment::Center);
        empty.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        empty.setMaximumNumberOfLines(2);
        content_root.addSubview(&empty);

        let separator = NSBox::initWithFrame(NSBox::alloc(mtm), NSRect::ZERO);
        separator.setBoxType(NSBoxType::Separator);
        content_root.addSubview(&separator);

        let composer = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(content_bounds.size.width - 154.0, 72.0),
            ),
        );
        composer.setEditable(true);
        composer.setSelectable(true);
        composer.setRichText(false);
        composer.setAllowsUndo(true);
        composer.setFont(Some(&NSFont::systemFontOfSize(
            self.ivars().font_sizes.get().agent,
        )));
        composer.setTextContainerInset(NSSize::new(10.0, 8.0));
        composer.setDelegate(Some(ProtocolObject::from_ref(self)));
        let composer_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), NSRect::ZERO);
        composer_scroll.setBorderType(NSBorderType::BezelBorder);
        composer_scroll.setDrawsBackground(true);
        composer_scroll.setHasVerticalScroller(true);
        composer_scroll.setAutohidesScrollers(true);
        composer_scroll.setDocumentView(Some(&composer));
        composer_scroll.setHidden(true);
        content_root.addSubview(&composer_scroll);

        let attach =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, true);
        attach.addItemWithTitle(&NSString::new());
        if let Some(item) = attach.lastItem()
            && let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("plus.circle"),
                Some(&NSString::from_str("Add context")),
            )
        {
            item.setImage(Some(&image));
        }
        if let Some(menu) = attach.menu() {
            for (title, symbol, action) in [
                (
                    "Attach Image or Audio…",
                    "photo.on.rectangle.angled",
                    sel!(attachAgentFiles:),
                ),
                (
                    "Reference Workspace File…",
                    "doc",
                    sel!(referenceAgentFile:),
                ),
                (
                    "Reference Workspace Folder…",
                    "folder",
                    sel!(referenceAgentFolder:),
                ),
            ] {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::new(),
                    )
                };
                unsafe { item.setTarget(Some(self)) };
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(title)),
                ) {
                    item.setImage(Some(&image));
                }
                menu.addItem(&item);
            }
        }
        attach.setControlSize(NSControlSize::Small);
        attach.setBordered(false);
        attach.setToolTip(Some(&NSString::from_str("Add context")));
        attach.setEnabled(false);
        attach.setHidden(true);
        content_root.addSubview(&attach);

        let attachment_tokens = NSTokenField::initWithFrame(NSTokenField::alloc(mtm), NSRect::ZERO);
        attachment_tokens.setEditable(false);
        attachment_tokens.setSelectable(true);
        attachment_tokens.setTokenStyle(NSTokenStyle::Rounded);
        attachment_tokens.setControlSize(NSControlSize::Small);
        attachment_tokens.setDrawsBackground(false);
        attachment_tokens.setBordered(false);
        attachment_tokens.setHidden(true);
        content_root.addSubview(&attachment_tokens);

        let clear_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("xmark.circle.fill"),
            Some(&NSString::from_str("Clear attachments")),
        )
        .expect("macOS provides xmark.circle.fill");
        let clear_attachments = unsafe {
            NSButton::buttonWithImage_target_action(
                &clear_image,
                Some(self),
                Some(sel!(clearAgentAttachments:)),
                mtm,
            )
        };
        clear_attachments.setBordered(false);
        clear_attachments.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        clear_attachments.setToolTip(Some(&NSString::from_str("Clear attachments")));
        clear_attachments.setHidden(true);
        content_root.addSubview(&clear_attachments);

        let send = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Send"),
                Some(self),
                Some(sel!(sendAgentMessage:)),
                mtm,
            )
        };
        send.setBezelStyle(NSBezelStyle::Push);
        send.setKeyEquivalent(&NSString::from_str("\r"));
        send.setEnabled(false);
        send.setHidden(true);
        content_root.addSubview(&send);

        let stop_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("stop.fill"),
            Some(&NSString::from_str("Stop Codex turn")),
        )
        .expect("macOS provides stop.fill");
        let stop = unsafe {
            NSButton::buttonWithImage_target_action(
                &stop_image,
                Some(self),
                Some(sel!(stopAgentTurn:)),
                mtm,
            )
        };
        stop.setBezelStyle(NSBezelStyle::Circular);
        stop.setToolTip(Some(&NSString::from_str("Stop Codex turn")));
        stop.setEnabled(false);
        stop.setHidden(true);
        content_root.addSubview(&stop);

        let terminal_panel = NSView::initWithFrame(NSView::alloc(mtm), content_root.bounds());
        terminal_panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        terminal_panel.setHidden(true);

        let terminal_stack = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(
                    content_bounds.size.width,
                    content_bounds.size.height.max(1.0),
                ),
            ),
        );
        terminal_stack.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        terminal_panel.addSubview(&terminal_stack);
        content_root.addSubview(&terminal_panel);

        AgentsUi {
            sidebar_root,
            new_chat,
            codex_cli,
            agy,
            history_search,
            history_scope,
            threads_scroll,
            threads_document,
            content_root,
            title,
            status,
            spinner,
            tools,
            thread_actions,
            model,
            reasoning,
            personality,
            service_tier,
            permissions,
            usage,
            usage_progress,
            transcript_scroll,
            transcript_table,
            empty,
            composer_scroll,
            composer,
            attach,
            attachment_tokens,
            clear_attachments,
            send,
            stop,
            separator,
            terminal_panel,
            terminal_stack,
            terminal_cards: RefCell::new(HashMap::new()),
            model_options: RefCell::new(Vec::new()),
            reasoning_options: RefCell::new(Vec::new()),
            personality_options: RefCell::new(Vec::new()),
            service_tier_options: RefCell::new(Vec::new()),
            permission_options: RefCell::new(Vec::new()),
            selected_model: RefCell::new(selected_model),
            selected_reasoning: RefCell::new(selected_reasoning),
            selected_personality: RefCell::new(selected_personality),
            selected_service_tier: RefCell::new(selected_service_tier),
            selected_permissions: RefCell::new(selected_permissions),
            selector_updates_suppressed: Cell::new(false),
            threads: RefCell::new(Vec::new()),
            history_query: RefCell::new(String::new()),
            history_archived: Cell::new(false),
            active_thread_id: RefCell::new(None),
            transcript_items: RefCell::new(Vec::new()),
            transcript_images: RefCell::new(HashMap::new()),
            transcript_image_order: RefCell::new(VecDeque::new()),
            transcript_image_in_flight: RefCell::new(HashMap::new()),
            transcript_image_errors: RefCell::new(HashMap::new()),
            attachments: RefCell::new(Vec::new()),
            generation: Cell::new(0),
            state: Cell::new(NativeAgentState::Closed),
        }
    }

}
