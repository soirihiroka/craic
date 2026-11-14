impl AppDelegate {
    fn make_agents_ui(&self, sidebar_bounds: NSRect, content_bounds: NSRect) -> AgentsUi {
        let mtm = self.mtm();
        let sidebar_root = NSView::initWithFrame(NSView::alloc(mtm), sidebar_bounds);
        sidebar_root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        sidebar_root.setHidden(true);

        let provider_icons = NATIVE_AGENT_PROVIDERS
            .iter()
            .filter_map(|provider| {
                native_agent_provider_icon(provider.provider_id())
                    .map(|image| (provider.provider_id().to_owned(), image))
            })
            .collect::<HashMap<_, _>>();
        let menu = NSMenu::new(mtm);
        for (index, provider) in NATIVE_AGENT_PROVIDERS.iter().enumerate() {
            let item = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(provider.label()),
                    Some(sel!(selectAgentProvider:)),
                    &NSString::new(),
                )
            };
            item.setTag(index as isize);
            item.setImage(
                provider_icons
                    .get(provider.provider_id())
                    .map(|image| &**image),
            );
            item.setState(if index == 0 {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            unsafe {
                item.setTarget(Some(self));
            }
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let check_updates = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Check for Provider Updates…"),
                Some(sel!(checkAgentProviderUpdates:)),
                &NSString::new(),
            )
        };
        check_updates.setTag(-1);
        unsafe {
            check_updates.setTarget(Some(self));
        }
        let new_chat = unsafe {
            NSComboButton::comboButtonWithTitle_menu_target_action(
                &NSString::from_str("Codex CLI"),
                Some(&menu),
                Some(self),
                Some(sel!(newAgentChat:)),
                mtm,
            )
        };
        new_chat.setStyle(NSComboButtonStyle::Split);
        new_chat.setImage(provider_icons.get("codex").map(|image| &**image));
        new_chat.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        new_chat.setControlSize(NSControlSize::Regular);
        new_chat.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMaxYMargin | NSAutoresizingMaskOptions::ViewMinXMargin,
        );
        new_chat.setToolTip(Some(&NSString::from_str("New Codex CLI chat")));
        sidebar_root.addSubview(&new_chat);

        let update_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("arrow.down.circle"),
            Some(&NSString::from_str("Update agent providers")),
        )
        .expect("macOS provides the provider-update SF Symbol");
        let update_button = unsafe {
            NSButton::buttonWithImage_target_action(
                &update_image,
                Some(self),
                Some(sel!(updateAgentProviders:)),
                mtm,
            )
        };
        update_button.setBordered(false);
        update_button.setControlSize(NSControlSize::Small);
        update_button.setToolTip(Some(&NSString::from_str("Update agent providers")));
        update_button.setHidden(true);
        sidebar_root.addSubview(&update_button);

        let history_status =
            NSTextField::labelWithString(&NSString::from_str("Loading history…"), mtm);
        history_status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        history_status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        history_status.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        sidebar_root.addSubview(&history_status);
        let history_tags_button =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, true);
        history_tags_button.addItemWithTitle(&NSString::from_str("Tags"));
        history_tags_button.setControlSize(NSControlSize::Small);
        history_tags_button.setToolTip(Some(&NSString::from_str("Filter by session tags")));
        history_tags_button.setHidden(true);
        sidebar_root.addSubview(&history_tags_button);

        let history_search = NSSearchField::initWithFrame(NSSearchField::alloc(mtm), NSRect::ZERO);
        history_search.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        history_search.setPlaceholderString(Some(&NSString::from_str("Search chats")));
        history_search.setToolTip(Some(&NSString::from_str("Search agent history")));
        history_search.setControlSize(NSControlSize::Small);
        history_search.setSendsSearchStringImmediately(true);
        history_search.setHidden(true);
        unsafe {
            history_search.setTarget(Some(self));
            history_search.setAction(Some(sel!(filterAgentThreads:)));
        }
        sidebar_root.addSubview(&history_search);
        let history_scope =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        history_scope.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        history_scope.addItemWithTitle(&NSString::from_str("More…"));
        history_scope.setToolTip(Some(&NSString::from_str("Load more agent history")));
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
                NSSize::new((sidebar_bounds.size.width - 24.0).max(1.0), 1.0),
            ),
        );
        let threads_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), NSRect::ZERO);
        threads_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        threads_scroll.setBorderType(NSBorderType::NoBorder);
        threads_scroll.setDrawsBackground(false);
        threads_scroll.setHasVerticalScroller(true);
        threads_scroll.setHasHorizontalScroller(false);
        threads_scroll.setAutohidesScrollers(true);
        threads_scroll.setAutomaticallyAdjustsContentInsets(false);
        threads_scroll.setDocumentView(Some(&threads_document));
        let history_clip = threads_scroll.contentView();
        history_clip.setPostsBoundsChangedNotifications(true);
        // SAFETY: This delegate and clip view are retained on AppKit's main thread.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(agentHistoryClipBoundsChanged:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&history_clip),
            );
        }
        sidebar_root.addSubview(&threads_scroll);

        let content_root = NSView::initWithFrame(NSView::alloc(mtm), content_bounds);
        content_root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content_root.setHidden(true);
        let empty = NSTextField::wrappingLabelWithString(
            &NSString::from_str("Start a new agent chat or select a session from history."),
            mtm,
        );
        empty.setAlignment(NSTextAlignment::Center);
        empty.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        empty.setMaximumNumberOfLines(2);
        content_root.addSubview(&empty);
        let terminal_panel = NSView::initWithFrame(NSView::alloc(mtm), content_root.bounds());
        terminal_panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        terminal_panel.setHidden(true);
        let terminal_stack = NSView::initWithFrame(NSView::alloc(mtm), content_root.bounds());
        terminal_stack.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        terminal_panel.addSubview(&terminal_stack);
        content_root.addSubview(&terminal_panel);

        AgentsUi {
            sidebar_root,
            new_chat,
            selected_provider: Cell::new(0),
            default_launch_attempted: Cell::new(false),
            provider_icons,
            history_status,
            history_rows: RefCell::new(Vec::new()),
            history_tags: RefCell::new(Vec::new()),
            selected_history_tags: RefCell::new(HashSet::new()),
            history_tags_button,
            history_request_id: Cell::new(0),
            history_loading: Cell::new(false),
            history_refresh_pending: Cell::new(false),
            history_loaded: Cell::new(false),
            history_limit: Cell::new(32),
            history_has_more: Cell::new(false),
            history_refresh_timer: RefCell::new(None),
            history_last_refresh_ms: Cell::new(0),
            history_started_at: Cell::new(None),
            history_cancellation: RefCell::new(None),
            history_request_filter: RefCell::new(Default::default()),
            history_search,
            history_scope,
            threads_scroll,
            threads_document,
            content_root,
            empty,
            terminal_panel,
            terminal_stack,
            terminal_cards: RefCell::new(HashMap::new()),
            threads: RefCell::new(Vec::new()),
            history_query: RefCell::new(String::new()),
            generation: Cell::new(0),
            update_button,
            provider_updates: RefCell::new(Vec::new()),
            updates_checking: Cell::new(false),
            updates_check_forced: Cell::new(false),
            updates_running: Cell::new(false),
            updates_checked_at: Cell::new(None),
            updates_generation: Cell::new(0),
            update_terminal_id: Cell::new(None),
        }
    }
}
