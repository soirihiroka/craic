impl AppDelegate {
    fn did_finish_launching(&self, notification: &NSNotification) {
        let mtm = self.mtm();
        let application = notification
            .object()
            .and_then(|object| object.downcast::<NSApplication>().ok())
            .expect("launch notification must belong to NSApplication");
        self.install_main_menu(&application);

        let content_rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        );
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                content_rect,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: Windows created without an NSWindowController must not release themselves.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Craic"));
        let window_frame_name = NSString::from_str(WINDOW_FRAME_AUTOSAVE);
        let restored_window_frame = window.setFrameUsingName(&window_frame_name);
        window.setFrameAutosaveName(&window_frame_name);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setMovableByWindowBackground(true);
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        let toolbar = NSToolbar::initWithIdentifier(
            NSToolbar::alloc(mtm),
            &NSString::from_str("dev.craic.Craic.main-toolbar"),
        );
        toolbar.setAllowsUserCustomization(false);
        toolbar.setAutosavesConfiguration(false);
        toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        toolbar.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.setToolbar(Some(&toolbar));
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.setContentMinSize(NSSize::new(980.0, 620.0));
        if let Some(appearance) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua })
        {
            application.setAppearance(Some(&appearance));
        }

        let root = window
            .contentView()
            .expect("window must have a content view");
        let bounds = root.bounds();

        let split_controller = NSSplitViewController::new(mtm);
        let split = split_controller.splitView();
        split.setFrame(bounds);
        split.setVertical(true);
        split.setDividerStyle(NSSplitViewDividerStyle::Thin);
        split.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        // NSSplitViewItem supplies the floating Liquid Glass sidebar on macOS 26.
        // A legacy Sidebar NSVisualEffectView here masks that system material.
        let sidebar = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(SIDEBAR_WIDTH, bounds.size.height),
            ),
        );
        sidebar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        let changes_search_panel = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, bounds.size.height - 38.0),
                NSSize::new(SIDEBAR_WIDTH, 38.0),
            ),
        );
        changes_search_panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let changes_search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            NSRect::new(
                NSPoint::new(10.0, 4.0),
                NSSize::new(SIDEBAR_WIDTH - 50.0, 30.0),
            ),
        );
        changes_search.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        changes_search.setPlaceholderString(Some(&NSString::from_str("Search changed files")));
        changes_search.setSendsSearchStringImmediately(true);
        unsafe {
            changes_search.setTarget(Some(self));
            changes_search.setAction(Some(sel!(filterChangedFiles:)));
        }
        changes_search_panel.addSubview(&changes_search);
        let close_changes_search = unsafe {
            NSButton::buttonWithImage_target_action(
                &NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("xmark"),
                    Some(&NSString::from_str("Close changed-file search")),
                )
                .expect("macOS provides the close search SF Symbol"),
                Some(self),
                Some(sel!(closeChangedFilesSearch:)),
                mtm,
            )
        };
        close_changes_search.setFrame(NSRect::new(
            NSPoint::new(SIDEBAR_WIDTH - 36.0, 5.0),
            NSSize::new(28.0, 28.0),
        ));
        close_changes_search.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        close_changes_search.setBezelStyle(NSBezelStyle::Circular);
        close_changes_search.setToolTip(Some(&NSString::from_str("Close search")));
        changes_search_panel.addSubview(&close_changes_search);
        changes_search_panel.setHidden(true);

        let selection_header = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, bounds.size.height - SELECTION_HEADER_HEIGHT),
                NSSize::new(SIDEBAR_WIDTH, SELECTION_HEADER_HEIGHT),
            ),
        );
        selection_header.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let select_all_check = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::new(),
                Some(self),
                Some(sel!(toggleAllChangedFiles:)),
                mtm,
            )
        };
        select_all_check.setFrame(NSRect::new(
            NSPoint::new(18.0, 7.0),
            NSSize::new(24.0, 28.0),
        ));
        select_all_check.setAllowsMixedState(true);
        select_all_check.setToolTip(Some(&NSString::from_str("Select all changed files")));
        selection_header.addSubview(&select_all_check);
        let select_all_label =
            NSTextField::labelWithString(&NSString::from_str("0 changed files"), mtm);
        select_all_label.setFrame(NSRect::new(
            NSPoint::new(48.0, 10.0),
            NSSize::new(SIDEBAR_WIDTH - 56.0, 22.0),
        ));
        select_all_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        select_all_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        selection_header.addSubview(&select_all_label);
        let changed_files_menu = NSMenu::new(mtm);
        for (title, action) in [
            ("Select All", sel!(selectAllChangedFilesFromMenu:)),
            ("Deselect All", sel!(deselectAllChangedFilesFromMenu:)),
        ] {
            let item = unsafe {
                changed_files_menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::new(),
                )
            };
            unsafe { item.setTarget(Some(self)) };
        }
        changed_files_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let stash_all = unsafe {
            changed_files_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Stash All Changes"),
                Some(sel!(stashAllChanges:)),
                &NSString::new(),
            )
        };
        unsafe { stash_all.setTarget(Some(self)) };
        let discard_all = unsafe {
            changed_files_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Discard All Changes…"),
                Some(sel!(confirmDiscardAllChanges:)),
                &NSString::new(),
            )
        };
        unsafe {
            discard_all.setTarget(Some(self));
            selection_header.setMenu(Some(&changed_files_menu));
            select_all_check.setMenu(Some(&changed_files_menu));
            select_all_label.setMenu(Some(&changed_files_menu));
        }
        let changes_top_cover = NSGlassEffectView::initWithFrame(
            NSGlassEffectView::alloc(mtm),
            NSRect::new(
                NSPoint::new(
                    SIDEBAR_WIDTH - 232.0,
                    bounds.size.height - SELECTION_HEADER_HEIGHT - 8.0,
                ),
                NSSize::new(220.0, SELECTION_HEADER_HEIGHT),
            ),
        );
        changes_top_cover.setStyle(NSGlassEffectViewStyle::Regular);
        changes_top_cover.setCornerRadius(SELECTION_HEADER_HEIGHT / 2.0);
        changes_top_cover.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let changes_top_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(220.0, SELECTION_HEADER_HEIGHT),
            ),
        );
        changes_top_content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        selection_header.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(220.0, SELECTION_HEADER_HEIGHT),
        ));
        selection_header.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        changes_top_content.addSubview(&selection_header);
        changes_top_cover.setContentView(Some(&changes_top_content));

        let changes_search_popup = NSGlassEffectView::initWithFrame(
            NSGlassEffectView::alloc(mtm),
            NSRect::new(
                NSPoint::new(
                    SIDEBAR_WIDTH - 372.0,
                    bounds.size.height - SELECTION_HEADER_HEIGHT - 62.0,
                ),
                NSSize::new(360.0, 46.0),
            ),
        );
        changes_search_popup.setStyle(NSGlassEffectViewStyle::Regular);
        changes_search_popup.setCornerRadius(23.0);
        changes_search_popup.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let changes_search_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(360.0, 46.0)),
        );
        changes_search_content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        changes_search_panel.setFrame(NSRect::new(
            NSPoint::new(0.0, 4.0),
            NSSize::new(360.0, 38.0),
        ));
        changes_search_panel.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        changes_search_content.addSubview(&changes_search_panel);
        changes_search_popup.setContentView(Some(&changes_search_content));
        changes_search_popup.setHidden(true);
        let commit_composer = CommitComposer::new(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(SIDEBAR_WIDTH, COMMIT_COMPOSER_HEIGHT),
            ),
            self,
            CommitComposerActions {
                select_author: sel!(selectCommitAuthor:),
                show_author_warning: sel!(showCommitAuthorWarning:),
                summary_changed: sel!(commitSummaryChanged:),
                generate_message: sel!(generateCommitMessage:),
                commit: sel!(commitChanges:),
            },
            mtm,
        );
        // SAFETY: AppDelegate implements NSTextFieldDelegate and outlives the retained composer.
        unsafe {
            commit_composer
                .summary_field
                .setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        commit_composer
            .description_view
            .setDelegate(Some(ProtocolObject::from_ref(self)));
        let changes_split = NSSplitView::initWithFrame(
            NSSplitView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(SIDEBAR_WIDTH, bounds.size.height),
            ),
        );
        changes_split.setVertical(false);
        changes_split.setDividerStyle(NSSplitViewDividerStyle::Thin);
        changes_split.setDelegate(Some(ProtocolObject::from_ref(self)));
        changes_split.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let browser_height = (bounds.size.height - COMMIT_COMPOSER_HEIGHT - 1.0).max(1.0);
        let changes_browser = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, COMMIT_COMPOSER_HEIGHT + 1.0),
                NSSize::new(SIDEBAR_WIDTH, browser_height),
            ),
        );
        changes_browser.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        let changes_list = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(
                    SIDEBAR_WIDTH - 20.0,
                    browser_height - SELECTION_HEADER_HEIGHT,
                ),
            ),
        );
        changes_list.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let changes_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(10.0, 0.0),
                NSSize::new(
                    SIDEBAR_WIDTH - 20.0,
                    browser_height - SELECTION_HEADER_HEIGHT,
                ),
            ),
        );
        changes_scroll.setBorderType(NSBorderType::NoBorder);
        changes_scroll.setDrawsBackground(false);
        changes_scroll.setAutomaticallyAdjustsContentInsets(false);
        changes_scroll.setHasVerticalScroller(true);
        changes_scroll.setAutohidesScrollers(true);
        changes_scroll.setDocumentView(Some(&changes_list));
        changes_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        changes_browser.addSubview(&changes_scroll);
        changes_split.addArrangedSubview(&changes_browser);
        changes_split.addArrangedSubview(&commit_composer.root);
        changes_split.setAutosaveName(Some(&NSString::from_str(CHANGES_SPLIT_AUTOSAVE)));
        changes_split.adjustSubviews();
        sidebar.addSubview(&changes_split);

        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(bounds.size.width - SIDEBAR_WIDTH, bounds.size.height),
            ),
        );
        content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        let diff_view = DiffMetalView::new(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(bounds.size.width - SIDEBAR_WIDTH, bounds.size.height),
            ),
            self.ivars().font_sizes.get().diff,
            mtm,
        );
        diff_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        diff_view.setHidden(true);
        content.addSubview(&diff_view);

        let image_preview = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            NSRect::new(
                NSPoint::new(24.0, 24.0),
                NSSize::new(
                    bounds.size.width - SIDEBAR_WIDTH - 48.0,
                    bounds.size.height - 48.0,
                ),
            ),
        );
        image_preview.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        image_preview.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        image_preview.setHidden(true);
        content.addSubview(&image_preview);

        let binary_preview = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(bounds.size.width - SIDEBAR_WIDTH, bounds.size.height),
            ),
        );
        binary_preview.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        binary_preview.setHidden(true);
        content.addSubview(&binary_preview);

        let diff_search_panel = NSGlassEffectView::initWithFrame(
            NSGlassEffectView::alloc(mtm),
            NSRect::new(
                NSPoint::new(
                    bounds.size.width - SIDEBAR_WIDTH - 416.0,
                    bounds.size.height - 58.0,
                ),
                NSSize::new(400.0, 46.0),
            ),
        );
        diff_search_panel.setStyle(NSGlassEffectViewStyle::Regular);
        diff_search_panel.setCornerRadius(23.0);
        diff_search_panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let diff_search_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 46.0)),
        );
        let diff_search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            NSRect::new(NSPoint::new(12.0, 8.0), NSSize::new(190.0, 30.0)),
        );
        diff_search.setPlaceholderString(Some(&NSString::from_str("Search diff")));
        diff_search.setSendsSearchStringImmediately(true);
        unsafe {
            diff_search.setTarget(Some(self));
            diff_search.setAction(Some(sel!(filterDiff:)));
        }
        diff_search_content.addSubview(&diff_search);
        let diff_search_status = NSTextField::labelWithString(&NSString::new(), mtm);
        diff_search_status.setFrame(NSRect::new(
            NSPoint::new(208.0, 13.0),
            NSSize::new(60.0, 20.0),
        ));
        diff_search_status.setAlignment(NSTextAlignment::Center);
        diff_search_status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        diff_search_status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        diff_search_content.addSubview(&diff_search_status);
        for (x, symbol, tooltip, action) in [
            (
                272.0,
                "chevron.up",
                "Previous match",
                sel!(previousDiffMatch:),
            ),
            (308.0, "chevron.down", "Next match", sel!(nextDiffMatch:)),
            (356.0, "xmark", "Close search", sel!(closeDiffSearch:)),
        ] {
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(tooltip)),
            )
            .expect("macOS provides diff search SF Symbols");
            let button = unsafe {
                NSButton::buttonWithImage_target_action(&image, Some(self), Some(action), mtm)
            };
            button.setFrame(NSRect::new(NSPoint::new(x, 7.0), NSSize::new(32.0, 32.0)));
            button.setBezelStyle(NSBezelStyle::Circular);
            button.setBordered(true);
            button.setToolTip(Some(&NSString::from_str(tooltip)));
            diff_search_content.addSubview(&button);
        }
        diff_search_panel.setContentView(Some(&diff_search_content));
        diff_search_panel.setHidden(true);
        content.addSubview(&diff_search_panel);
        diff_view.attach_search_panel(&diff_search_panel, &diff_search);

        let diff_spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(
                NSPoint::new(
                    (bounds.size.width - SIDEBAR_WIDTH) / 2.0 - 12.0,
                    bounds.size.height / 2.0 - 12.0,
                ),
                NSSize::new(24.0, 24.0),
            ),
        );
        diff_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        diff_spinner.setControlSize(NSControlSize::Regular);
        diff_spinner.setIndeterminate(true);
        diff_spinner.setDisplayedWhenStopped(false);
        diff_spinner.setHidden(true);
        diff_spinner.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewMaxXMargin
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        content.addSubview(&diff_spinner);

        let empty_state =
            NSTextField::labelWithString(&NSString::from_str("Open a workspace to begin"), mtm);
        empty_state.setFrame(NSRect::new(
            NSPoint::new(0.0, bounds.size.height / 2.0 - 20.0),
            NSSize::new(bounds.size.width - SIDEBAR_WIDTH, 40.0),
        ));
        empty_state.setAlignment(NSTextAlignment::Center);
        empty_state.setFont(Some(&NSFont::systemFontOfSize(17.0)));
        empty_state.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        empty_state.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        content.addSubview(&empty_state);

        let content_home_root = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(bounds.size.width - SIDEBAR_WIDTH, bounds.size.height),
            ),
        );
        content_home_root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content_home_root.setHidden(true);
        content.addSubview(&content_home_root);

        let content_home_title =
            NSTextField::labelWithString(&NSString::from_str("No local changes"), mtm);
        content_home_title.setAlignment(NSTextAlignment::Left);
        content_home_title.setFont(Some(&NSFont::boldSystemFontOfSize(24.0)));
        content_home_root.addSubview(&content_home_title);
        let content_home_subtitle = NSTextField::labelWithString(&NSString::new(), mtm);
        content_home_subtitle.setAlignment(NSTextAlignment::Left);
        content_home_subtitle.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        content_home_subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
        content_home_root.addSubview(&content_home_subtitle);

        let suggestion_card = |title: &str, subtitle: &str, button_title: &str, action: Sel| {
            let card = NSBox::initWithFrame(
                NSBox::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(640.0, 72.0)),
            );
            card.setBoxType(NSBoxType::Custom);
            card.setBorderWidth(1.0);
            card.setBorderColor(&NSColor::separatorColor());
            card.setFillColor(&NSColor::controlBackgroundColor());
            card.setCornerRadius(10.0);
            let text_group = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(480.0, 36.0)),
            );
            text_group.setTranslatesAutoresizingMaskIntoConstraints(false);
            let title = NSTextField::labelWithString(&NSString::from_str(title), mtm);
            title.setFrame(NSRect::new(
                NSPoint::new(0.0, 19.0),
                NSSize::new(480.0, 17.0),
            ));
            title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            title.setFont(Some(&NSFont::boldSystemFontOfSize(13.5)));
            title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            text_group.addSubview(&title);
            let subtitle = NSTextField::labelWithString(&NSString::from_str(subtitle), mtm);
            subtitle.setFrame(NSRect::new(
                NSPoint::ZERO,
                NSSize::new(480.0, 15.0),
            ));
            subtitle.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            subtitle.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
            subtitle.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            text_group.addSubview(&subtitle);
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(button_title),
                    Some(self),
                    Some(action),
                    mtm,
                )
            };
            button.setTranslatesAutoresizingMaskIntoConstraints(false);
            button.setBezelStyle(NSBezelStyle::Push);
            button.setControlSize(NSControlSize::Regular);
            card.addSubview(&text_group);
            card.addSubview(&button);
            text_group
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&card.leadingAnchor(), 18.0)
                .setActive(true);
            text_group
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&button.leadingAnchor(), -12.0)
                .setActive(true);
            text_group
                .centerYAnchor()
                .constraintEqualToAnchor(&card.centerYAnchor())
                .setActive(true);
            text_group
                .heightAnchor()
                .constraintEqualToConstant(36.0)
                .setActive(true);
            button
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&card.trailingAnchor(), -18.0)
                .setActive(true);
            button
                .centerYAnchor()
                .constraintEqualToAnchor(&card.centerYAnchor())
                .setActive(true);
            button
                .widthAnchor()
                .constraintEqualToConstant(102.0)
                .setActive(true);
            button
                .heightAnchor()
                .constraintEqualToConstant(32.0)
                .setActive(true);
            (card, title, subtitle, button)
        };

        let (git_card, git_title, git_subtitle, content_home_action) = suggestion_card(
            "Repository action",
            "Synchronize this branch with its remote.",
            "Fetch",
            sel!(fetchRemote:),
        );
        git_card.setHidden(true);
        let (initialize_card, _, _, content_home_initialize) = suggestion_card(
            "Initialize Git Repository",
            "Create a Git repository under the current workspace.",
            "Initialize",
            sel!(initializeRepositorySuggestion:),
        );
        initialize_card.setHidden(true);
        let (editor_card, _, _, content_home_editor) = suggestion_card(
            "Open in editor",
            "Jump into the project files.",
            "Open",
            sel!(openRepositorySuggestionInEditor:),
        );
        let (terminal_card, _, _, content_home_terminal) = suggestion_card(
            "Open in Ghostty",
            "Open the repository in an external Ghostty window.",
            "Open",
            sel!(openRepositorySuggestionInGhostty:),
        );
        let (files_card, _, _, content_home_files) = suggestion_card(
            "Open in Files",
            "Open the repository folder in Finder.",
            "Show",
            sel!(showRepositorySuggestionInFinder:),
        );
        let (remote_card, _, _, content_home_remote) = suggestion_card(
            "View on GitHub",
            "Open the remote repository.",
            "View",
            sel!(openRepositorySuggestionRemote:),
        );
        let content_home_cards = vec![
            git_card,
            initialize_card.clone(),
            editor_card,
            terminal_card,
            files_card,
            remote_card,
        ];
        for card in &content_home_cards {
            content_home_root.addSubview(card);
        }

        let history = self.make_history_ui(sidebar.bounds(), content.bounds());
        let files = self.make_files_ui(sidebar.bounds(), content.bounds());
        let containers = self.make_containers_ui(sidebar.bounds(), content.bounds());
        let agents = self.make_agents_ui(sidebar.bounds(), content.bounds());
        sidebar.addSubview(&history.sidebar_root);
        sidebar.addSubview(&files.sidebar_root);
        sidebar.addSubview(&containers.sidebar_root);
        sidebar.addSubview(&agents.sidebar_root);
        sidebar.addSubview(&changes_top_cover);
        content.addSubview(&history.content_root);
        content.addSubview(&files.content_root);
        content.addSubview(&containers.content_root);
        content.addSubview(&agents.content_root);

        let toast_width = 420.0_f64.min((content.bounds().size.width - 48.0).max(220.0));
        let toast = NSGlassEffectView::initWithFrame(
            NSGlassEffectView::alloc(mtm),
            NSRect::new(
                NSPoint::new((content.bounds().size.width - toast_width) / 2.0, 24.0),
                NSSize::new(toast_width, 44.0),
            ),
        );
        toast.setStyle(NSGlassEffectViewStyle::Regular);
        toast.setCornerRadius(22.0);
        toast.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewMaxXMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        let toast_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(toast_width, 44.0)),
        );
        toast_content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let toast_label = NSTextField::labelWithString(&NSString::new(), mtm);
        toast_label.setFrame(NSRect::new(
            NSPoint::new(18.0, 12.0),
            NSSize::new(toast_width - 36.0, 20.0),
        ));
        toast_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        toast_label.setAlignment(NSTextAlignment::Center);
        toast_label.setFont(Some(&NSFont::systemFontOfSize(12.5)));
        toast_label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        toast_content.addSubview(&toast_label);
        toast.setContentView(Some(&toast_content));
        toast.setHidden(true);
        content.addSubview(&toast);

        let terminal_panel_height = 340.0;
        let terminal_panel = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(content.bounds().size.width, terminal_panel_height),
            ),
        );
        terminal_panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let terminal_header = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, terminal_panel_height - 38.0),
                NSSize::new(content.bounds().size.width, 38.0),
            ),
        );
        terminal_header.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let new_terminal = unsafe {
            NSButton::buttonWithImage_target_action(
                &NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("plus"),
                    Some(&NSString::from_str("New terminal session")),
                )
                .expect("macOS provides the new terminal SF Symbol"),
                Some(self),
                Some(sel!(newTerminalSession:)),
                mtm,
            )
        };
        new_terminal.setFrame(NSRect::new(NSPoint::new(8.0, 5.0), NSSize::new(28.0, 28.0)));
        new_terminal.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        new_terminal.setToolTip(Some(&NSString::from_str("New terminal session")));
        terminal_header.addSubview(&new_terminal);

        let terminal_tab_strip = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(1.0, 32.0)),
        );
        let terminal_tab_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(42.0, 3.0),
                NSSize::new((content.bounds().size.width - 50.0).max(1.0), 32.0),
            ),
        );
        terminal_tab_scroll.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        terminal_tab_scroll.setBorderType(NSBorderType::NoBorder);
        terminal_tab_scroll.setDrawsBackground(false);
        terminal_tab_scroll.setHasHorizontalScroller(true);
        terminal_tab_scroll.setHasVerticalScroller(false);
        terminal_tab_scroll.setAutohidesScrollers(true);
        terminal_tab_scroll.setDocumentView(Some(&terminal_tab_strip));
        terminal_header.addSubview(&terminal_tab_scroll);
        terminal_panel.addSubview(&terminal_header);

        let terminal_search_panel = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, terminal_panel_height - 76.0),
                NSSize::new(content.bounds().size.width, 38.0),
            ),
        );
        terminal_search_panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        terminal_search_panel.setHidden(true);
        let terminal_search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            NSRect::new(
                NSPoint::new(8.0, 5.0),
                NSSize::new((content.bounds().size.width - 370.0).max(120.0), 28.0),
            ),
        );
        terminal_search.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        terminal_search.setPlaceholderString(Some(&NSString::from_str("Search Terminal")));
        terminal_search.setSendsSearchStringImmediately(true);
        terminal_search.setControlSize(NSControlSize::Small);
        unsafe {
            terminal_search.setTarget(Some(self));
            terminal_search.setAction(Some(sel!(filterTerminal:)));
        }
        terminal_search_panel.addSubview(&terminal_search);

        let terminal_search_status = NSTextField::labelWithString(&NSString::new(), mtm);
        terminal_search_status.setFrame(NSRect::new(
            NSPoint::new(content.bounds().size.width - 352.0, 10.0),
            NSSize::new(72.0, 18.0),
        ));
        terminal_search_status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        terminal_search_status.setAlignment(NSTextAlignment::Right);
        terminal_search_status.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        terminal_search_status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        terminal_search_panel.addSubview(&terminal_search_status);

        let terminal_search_case = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Aa"),
                Some(self),
                Some(sel!(toggleTerminalSearchOption:)),
                mtm,
            )
        };
        terminal_search_case.setFrame(NSRect::new(
            NSPoint::new(content.bounds().size.width - 274.0, 5.0),
            NSSize::new(34.0, 28.0),
        ));
        terminal_search_case.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        terminal_search_case.setButtonType(NSButtonType::Toggle);
        terminal_search_case.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        terminal_search_case.setControlSize(NSControlSize::Small);
        terminal_search_case.setToolTip(Some(&NSString::from_str("Match case")));
        terminal_search_panel.addSubview(&terminal_search_case);

        let terminal_search_word = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Word"),
                Some(self),
                Some(sel!(toggleTerminalSearchOption:)),
                mtm,
            )
        };
        terminal_search_word.setFrame(NSRect::new(
            NSPoint::new(content.bounds().size.width - 238.0, 5.0),
            NSSize::new(44.0, 28.0),
        ));
        terminal_search_word.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        terminal_search_word.setButtonType(NSButtonType::Toggle);
        terminal_search_word.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        terminal_search_word.setControlSize(NSControlSize::Small);
        terminal_search_word.setToolTip(Some(&NSString::from_str("Match whole word")));
        terminal_search_panel.addSubview(&terminal_search_word);

        let terminal_search_regex = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(".*"),
                Some(self),
                Some(sel!(toggleTerminalSearchOption:)),
                mtm,
            )
        };
        terminal_search_regex.setFrame(NSRect::new(
            NSPoint::new(content.bounds().size.width - 192.0, 5.0),
            NSSize::new(34.0, 28.0),
        ));
        terminal_search_regex.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        terminal_search_regex.setButtonType(NSButtonType::Toggle);
        terminal_search_regex.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        terminal_search_regex.setControlSize(NSControlSize::Small);
        terminal_search_regex.setToolTip(Some(&NSString::from_str("Use regular expression")));
        terminal_search_panel.addSubview(&terminal_search_regex);

        for (offset, symbol, tooltip, action) in [
            (
                152.0,
                "chevron.up",
                "Previous match",
                sel!(previousTerminalMatch:),
            ),
            (
                118.0,
                "chevron.down",
                "Next match",
                sel!(nextTerminalMatch:),
            ),
            (
                42.0,
                "xmark",
                "Close terminal search",
                sel!(closeTerminalSearch:),
            ),
        ] {
            let button = unsafe {
                NSButton::buttonWithImage_target_action(
                    &NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str(symbol),
                        Some(&NSString::from_str(tooltip)),
                    )
                    .expect("macOS provides terminal search SF Symbols"),
                    Some(self),
                    Some(action),
                    mtm,
                )
            };
            button.setFrame(NSRect::new(
                NSPoint::new(content.bounds().size.width - offset, 5.0),
                NSSize::new(30.0, 28.0),
            ));
            button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
            button.setControlSize(NSControlSize::Small);
            button.setToolTip(Some(&NSString::from_str(tooltip)));
            terminal_search_panel.addSubview(&button);
        }
        terminal_panel.addSubview(&terminal_search_panel);

        let terminal_stack = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(content.bounds().size.width, terminal_panel_height - 38.0),
            ),
        );
        terminal_stack.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        terminal_panel.addSubview(&terminal_stack);
        terminal_panel.setHidden(true);

        let content_split = NSSplitView::initWithFrame(
            NSSplitView::alloc(mtm),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(bounds.size.width - SIDEBAR_WIDTH, bounds.size.height),
            ),
        );
        content_split.setVertical(false);
        content_split.setDividerStyle(NSSplitViewDividerStyle::Thin);
        content_split.setDelegate(Some(ProtocolObject::from_ref(self)));
        content_split.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content_split.addArrangedSubview(&content);
        content_split.addArrangedSubview(&terminal_panel);
        content_split.setAutosaveName(Some(&NSString::from_str(TERMINAL_SPLIT_AUTOSAVE)));
        content_split.adjustSubviews();

        let sidebar_controller = NSViewController::new(mtm);
        sidebar_controller.setView(&sidebar);
        let sidebar_item = NSSplitViewItem::sidebarWithViewController(&sidebar_controller);
        sidebar_item.setCanCollapse(false);
        sidebar_item.setMinimumThickness(320.0);
        sidebar_item.setMaximumThickness(SIDEBAR_MAX_WIDTH);
        let content_controller = NSViewController::new(mtm);
        content_controller.setView(&content_split);
        let content_item = NSSplitViewItem::splitViewItemWithViewController(&content_controller);
        split_controller.addSplitViewItem(&sidebar_item);
        split_controller.addSplitViewItem(&content_item);
        split.setAutosaveName(Some(&NSString::from_str(MAIN_SPLIT_AUTOSAVE)));
        // NSSplitViewController must own the window content view. Adding only its
        // managed NSSplitView as an arbitrary subview bypasses the controller's
        // lazy loading and item layout, which can leave every pane at zero size.
        // The window retains the controller and installs its managed view.
        window.setContentViewController(Some(&split_controller));

        let changes_edge_container = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(SIDEBAR_WIDTH, SELECTION_HEADER_HEIGHT + 16.0),
            ),
        );
        changes_edge_container.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let changes_edge_height = changes_edge_container
            .heightAnchor()
            .constraintEqualToConstant(SELECTION_HEADER_HEIGHT + 16.0);
        changes_edge_height.setActive(true);
        changes_edge_container.addSubview(&changes_search_popup);
        if let Some(accessory_class) = AnyClass::get(c"NSSplitViewItemAccessoryViewController") {
            let changes_edge_accessory = unsafe {
                let allocated: *mut AnyObject = msg_send![accessory_class, alloc];
                let initialized: *mut AnyObject = msg_send![allocated, init];
                Retained::from_raw(initialized)
            }
            .expect("Changes edge accessory must initialize");
            unsafe {
                let _: () = msg_send![&*changes_edge_accessory, setView: &*changes_edge_container];
                let _: () = msg_send![
                    &*changes_edge_accessory,
                    setAutomaticallyAppliesContentInsets: false
                ];
                if let Some(style_class) = AnyClass::get(c"NSScrollEdgeEffectStyle") {
                    let soft_style: *mut AnyObject = msg_send![style_class, softStyle];
                    if !soft_style.is_null() {
                        let _: () = msg_send![
                            &*changes_edge_accessory,
                            setPreferredScrollEdgeEffectStyle: soft_style
                        ];
                    }
                }
                let _: () = msg_send![
                    &*sidebar_item,
                    addTopAlignedAccessoryViewController: &*changes_edge_accessory
                ];
            }
            self.ivars()
                .changes_edge_accessory
                .set(changes_edge_accessory)
                .expect("Changes edge accessory is initialized once");
        } else {
            log::warn!("native scroll-edge accessory unavailable; omitting Changes edge effect");
            sidebar.addSubview(&changes_edge_container);
        }

        self.ivars()
            .commit_composer
            .set(commit_composer)
            .unwrap_or_else(|_| panic!("commit composer is initialized once"));
        self.ivars()
            .split_controller
            .set(split_controller)
            .expect("split controller is initialized once");
        self.ivars()
            .sidebar
            .set(sidebar)
            .unwrap_or_else(|_| panic!("sidebar is initialized once"));
        self.ivars()
            .changes_split
            .set(changes_split)
            .expect("Changes split is initialized once");
        self.ivars()
            .changes_browser
            .set(changes_browser)
            .expect("Changes browser pane is initialized once");
        self.ivars()
            .changes_edge_container
            .set(changes_edge_container)
            .expect("Changes edge container is initialized once");
        self.ivars()
            .changes_edge_height
            .set(changes_edge_height)
            .expect("Changes edge height is initialized once");
        self.ivars()
            .changes_top_cover
            .set(changes_top_cover)
            .expect("Changes top cover is initialized once");
        self.ivars()
            .changes_search_popup
            .set(changes_search_popup)
            .expect("Changes search popup is initialized once");
        self.ivars()
            .content
            .set(content)
            .unwrap_or_else(|_| panic!("content view is initialized once"));
        self.ivars()
            .toast
            .set(toast)
            .expect("native toast is initialized once");
        self.ivars()
            .toast_label
            .set(toast_label)
            .expect("native toast label is initialized once");
        self.ivars()
            .content_split
            .set(content_split)
            .expect("content split is initialized once");
        self.ivars()
            .terminal_panel
            .set(terminal_panel)
            .expect("terminal panel is initialized once");
        self.ivars()
            .terminal_tab_strip
            .set(terminal_tab_strip)
            .expect("terminal tab strip is initialized once");
        self.ivars()
            .terminal_stack
            .set(terminal_stack)
            .expect("terminal stack is initialized once");
        self.ivars()
            .terminal_search_panel
            .set(terminal_search_panel)
            .expect("terminal search panel is initialized once");
        self.ivars()
            .terminal_search
            .set(terminal_search)
            .expect("terminal search field is initialized once");
        self.ivars()
            .terminal_search_status
            .set(terminal_search_status)
            .expect("terminal search status is initialized once");
        self.ivars()
            .terminal_search_case
            .set(terminal_search_case)
            .expect("terminal search case option is initialized once");
        self.ivars()
            .terminal_search_word
            .set(terminal_search_word)
            .expect("terminal search word option is initialized once");
        self.ivars()
            .terminal_search_regex
            .set(terminal_search_regex)
            .expect("terminal search regex option is initialized once");
        self.ivars()
            .changes_search_panel
            .set(changes_search_panel)
            .expect("changed-file search panel is initialized once");
        self.ivars()
            .changes_search
            .set(changes_search)
            .expect("changed-file search field is initialized once");
        self.ivars()
            .selection_header
            .set(selection_header)
            .expect("selection header is initialized once");
        self.ivars()
            .select_all_check
            .set(select_all_check)
            .expect("select-all checkbox is initialized once");
        self.ivars()
            .select_all_label
            .set(select_all_label)
            .expect("select-all label is initialized once");
        self.ivars()
            .content_empty
            .set(empty_state)
            .expect("content empty state is initialized once");
        self.ivars()
            .content_home_root
            .set(content_home_root)
            .expect("content home root is initialized once");
        self.ivars()
            .content_home_title
            .set(content_home_title)
            .expect("content home title is initialized once");
        self.ivars()
            .content_home_subtitle
            .set(content_home_subtitle)
            .expect("content home subtitle is initialized once");
        self.ivars()
            .content_home_cards
            .set(content_home_cards)
            .expect("content home cards are initialized once");
        self.ivars()
            .content_home_git_title
            .set(git_title)
            .expect("content home Git title is initialized once");
        self.ivars()
            .content_home_git_subtitle
            .set(git_subtitle)
            .expect("content home Git subtitle is initialized once");
        self.ivars()
            .content_home_action
            .set(content_home_action)
            .expect("content home action is initialized once");
        self.ivars()
            .content_home_initialize_card
            .set(initialize_card)
            .expect("content home initialize card is initialized once");
        self.ivars()
            .content_home_initialize
            .set(content_home_initialize)
            .expect("content home initialize action is initialized once");
        self.ivars()
            .content_home_editor
            .set(content_home_editor)
            .expect("content home editor action is initialized once");
        self.ivars()
            .content_home_terminal
            .set(content_home_terminal)
            .expect("content home terminal action is initialized once");
        self.ivars()
            .content_home_files
            .set(content_home_files)
            .expect("content home Files action is initialized once");
        self.ivars()
            .content_home_remote
            .set(content_home_remote)
            .expect("content home remote action is initialized once");
        self.ivars()
            .changes_list
            .set(changes_list)
            .expect("changes list is initialized once");
        self.ivars()
            .changes_scroll
            .set(changes_scroll)
            .expect("changes scroller is initialized once");
        self.ivars()
            .diff_view
            .set(diff_view)
            .unwrap_or_else(|_| panic!("diff Metal view is initialized once"));
        self.ivars()
            .image_preview
            .set(image_preview)
            .expect("image preview is initialized once");
        self.ivars()
            .binary_preview
            .set(binary_preview)
            .expect("binary comparison preview is initialized once");
        self.ivars()
            .diff_search_panel
            .set(diff_search_panel)
            .unwrap_or_else(|_| panic!("diff search panel is initialized once"));
        self.ivars()
            .diff_search
            .set(diff_search)
            .unwrap_or_else(|_| panic!("diff search field is initialized once"));
        self.ivars()
            .diff_search_status
            .set(diff_search_status)
            .unwrap_or_else(|_| panic!("diff search status is initialized once"));
        self.ivars()
            .diff_spinner
            .set(diff_spinner)
            .expect("diff spinner is initialized once");
        self.ivars()
            .history
            .set(history)
            .unwrap_or_else(|_| panic!("history UI is initialized once"));
        self.ivars()
            .files
            .set(files)
            .unwrap_or_else(|_| panic!("Files UI is initialized once"));
        self.ivars()
            .containers
            .set(containers)
            .unwrap_or_else(|_| panic!("Containers UI is initialized once"));
        self.ivars()
            .agents
            .set(agents)
            .unwrap_or_else(|_| panic!("Agents UI is initialized once"));
        if !restored_window_frame {
            window.center();
        }
        window.makeKeyAndOrderFront(None);
        self.ivars()
            .window
            .set(window)
            .expect("window is initialized once");
        self.update_native_renderer_occlusion();
        self.layout_sidebar();
        self.layout_content();

        if let Some(message) = self.ivars().startup_error.borrow_mut().take()
            && let Some(window) = self.ivars().window.get()
        {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Unable to Open Startup Workspace"));
            alert.setInformativeText(&NSString::from_str(&message));
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.beginSheetModalForWindow_completionHandler(window, None);
        }

        application.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        application.activate();
        log::info!("native macOS application window launched");
    }

}
