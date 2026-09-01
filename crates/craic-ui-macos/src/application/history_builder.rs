impl AppDelegate {
    fn make_history_ui(&self, sidebar_bounds: NSRect, content_bounds: NSRect) -> HistoryUi {
        let mtm = self.mtm();
        let sidebar_root = NSView::initWithFrame(NSView::alloc(mtm), sidebar_bounds);
        sidebar_root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        sidebar_root.setHidden(true);

        let search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            NSRect::new(
                NSPoint::new(12.0, sidebar_bounds.size.height - 42.0),
                NSSize::new((sidebar_bounds.size.width - 24.0).max(1.0), 30.0),
            ),
        );
        search.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        search.setPlaceholderString(Some(&NSString::from_str("Search history")));
        search.setSendsSearchStringImmediately(true);
        search.setHidden(true);
        unsafe {
            search.setTarget(Some(self));
            search.setAction(Some(sel!(filterHistory:)));
        }
        sidebar_root.addSubview(&search);

        let table = NSTableView::initWithFrame(
            NSTableView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(sidebar_bounds.size.width.max(1.0), 1.0),
            ),
        );
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(mtm),
            &NSUserInterfaceItemIdentifier::from_str("history.commit"),
        );
        column.setWidth(sidebar_bounds.size.width.max(1.0));
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        table.setRowHeight(54.0);
        table.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        table.setUsesAlternatingRowBackgroundColors(false);
        table.setAllowsEmptySelection(true);
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let history_menu = NSMenu::new(mtm);
        history_menu.setAutoenablesItems(false);
        history_menu.setDelegate(Some(ProtocolObject::from_ref(self)));
        // SAFETY: The retained menu targets this main-thread delegate for the table's lifetime.
        unsafe { table.setMenu(Some(&history_menu)) };
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(8.0, 50.0),
                NSSize::new(
                    (sidebar_bounds.size.width - 16.0).max(1.0),
                    (sidebar_bounds.size.height - 100.0).max(1.0),
                ),
            ),
        );
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(false);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&table));
        let clip = scroll.contentView();
        clip.setPostsBoundsChangedNotifications(true);
        // SAFETY: AppDelegate implements historyClipBoundsChanged:, the observed clip view
        // lives for the app window's lifetime, and delivery remains on AppKit's main thread.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(historyClipBoundsChanged:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
            );
        }
        sidebar_root.addSubview(&scroll);

        let loading_spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(24.0, 24.0)),
        );
        loading_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        loading_spinner.setControlSize(NSControlSize::Regular);
        loading_spinner.setIndeterminate(true);
        loading_spinner.setDisplayedWhenStopped(false);
        scroll.addSubview(&loading_spinner);

        let status = NSTextField::labelWithString(&NSString::from_str("No commits loaded"), mtm);
        status.setFrame(NSRect::new(
            NSPoint::new(12.0, sidebar_bounds.size.height / 2.0 - 32.0),
            NSSize::new((sidebar_bounds.size.width - 40.0).max(1.0), 20.0),
        ));
        status.setAlignment(NSTextAlignment::Center);
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        scroll.addSubview(&status);

        let content_root = NSView::initWithFrame(NSView::alloc(mtm), content_bounds);
        content_root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content_root.setHidden(true);
        let title = NSTextField::labelWithString(&NSString::from_str("Select a commit"), mtm);
        title.setFrame(NSRect::new(
            NSPoint::new(20.0, content_bounds.size.height - 45.0),
            NSSize::new((content_bounds.size.width - 260.0).max(1.0), 26.0),
        ));
        title.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        title.setFont(Some(&NSFont::boldSystemFontOfSize(17.0)));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        title.setSelectable(true);
        content_root.addSubview(&title);

        let avatar_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("person.crop.circle.fill"),
            Some(&NSString::from_str("Commit author")),
        )
        .expect("macOS provides the author SF Symbol");
        let avatar = NSImageView::imageViewWithImage(&avatar_image, mtm);
        avatar.setFrame(NSRect::new(
            NSPoint::new(20.0, content_bounds.size.height - 136.0),
            NSSize::new(32.0, 32.0),
        ));
        avatar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        avatar.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        avatar.setWantsLayer(true);
        if let Some(layer) = avatar.layer() {
            layer.setCornerRadius(16.0);
            layer.setMasksToBounds(true);
        }
        avatar.setHidden(true);
        content_root.addSubview(&avatar);

        let metadata = NSTextField::labelWithString(&NSString::new(), mtm);
        metadata.setFrame(NSRect::new(
            NSPoint::new(60.0, content_bounds.size.height - 130.0),
            NSSize::new((content_bounds.size.width - 196.0).max(1.0), 20.0),
        ));
        metadata.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        metadata.setTextColor(Some(&NSColor::secondaryLabelColor()));
        metadata.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        metadata.setSelectable(true);
        content_root.addSubview(&metadata);

        let added = NSTextField::labelWithString(&NSString::new(), mtm);
        added.setFrame(NSRect::new(
            NSPoint::new(
                content_bounds.size.width - 130.0,
                content_bounds.size.height - 130.0,
            ),
            NSSize::new(54.0, 20.0),
        ));
        added.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        added.setAlignment(NSTextAlignment::Right);
        added.setTextColor(Some(&NSColor::systemGreenColor()));
        added.setHidden(true);
        content_root.addSubview(&added);

        let deleted = NSTextField::labelWithString(&NSString::new(), mtm);
        deleted.setFrame(NSRect::new(
            NSPoint::new(
                content_bounds.size.width - 72.0,
                content_bounds.size.height - 130.0,
            ),
            NSSize::new(54.0, 20.0),
        ));
        deleted.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        deleted.setAlignment(NSTextAlignment::Right);
        deleted.setTextColor(Some(&NSColor::systemRedColor()));
        deleted.setHidden(true);
        content_root.addSubview(&deleted);

        let comment = NSTextField::wrappingLabelWithString(&NSString::new(), mtm);
        comment.setFrame(NSRect::new(
            NSPoint::new(20.0, content_bounds.size.height - 88.0),
            NSSize::new((content_bounds.size.width - 40.0).max(1.0), 38.0),
        ));
        comment.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        comment.setMaximumNumberOfLines(2);
        comment.setTextColor(Some(&NSColor::secondaryLabelColor()));
        comment.setSelectable(true);
        content_root.addSubview(&comment);

        let copy_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("doc.on.doc"),
            Some(&NSString::from_str("Copy full commit hash")),
        )
        .expect("macOS provides the copy SF Symbol");
        let copy_hash = unsafe {
            NSButton::buttonWithImage_target_action(
                &copy_image,
                Some(self),
                Some(sel!(copyHistoryHash:)),
                mtm,
            )
        };
        copy_hash.setFrame(NSRect::new(
            NSPoint::new(
                content_bounds.size.width - 82.0,
                content_bounds.size.height - 45.0,
            ),
            NSSize::new(32.0, 28.0),
        ));
        copy_hash.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        copy_hash.setBezelStyle(NSBezelStyle::AccessoryBar);
        copy_hash.setToolTip(Some(&NSString::from_str("Copy full commit hash")));
        copy_hash.setEnabled(false);
        content_root.addSubview(&copy_hash);
        let remote_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("arrow.up.right.square"),
            Some(&NSString::from_str("Open commit on remote")),
        )
        .expect("macOS provides the open-remote SF Symbol");
        let open_remote = unsafe {
            NSButton::buttonWithImage_target_action(
                &remote_image,
                Some(self),
                Some(sel!(openHistoryRemote:)),
                mtm,
            )
        };
        open_remote.setFrame(NSRect::new(
            NSPoint::new(
                content_bounds.size.width - 44.0,
                content_bounds.size.height - 45.0,
            ),
            NSSize::new(32.0, 28.0),
        ));
        open_remote.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        open_remote.setBezelStyle(NSBezelStyle::AccessoryBar);
        open_remote.setToolTip(Some(&NSString::from_str("Open commit on remote")));
        open_remote.setEnabled(false);
        content_root.addSubview(&open_remote);

        let files_table = NSTableView::initWithFrame(
            NSTableView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 1.0)),
        );
        let files_column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(mtm),
            &NSUserInterfaceItemIdentifier::from_str("history.file"),
        );
        files_column.setWidth(280.0);
        files_table.addTableColumn(&files_column);
        files_table.setHeaderView(None);
        files_table.setRowHeight(32.0);
        files_table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        files_table.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        files_table.setAllowsEmptySelection(true);
        files_table.setAllowsMultipleSelection(false);
        unsafe {
            files_table.setDataSource(Some(ProtocolObject::from_ref(self)));
            files_table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let files_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(280.0, (content_bounds.size.height - 126.0).max(1.0)),
            ),
        );
        files_scroll.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        files_scroll.setBorderType(NSBorderType::NoBorder);
        files_scroll.setDrawsBackground(false);
        files_scroll.setHasVerticalScroller(true);
        files_scroll.setHasHorizontalScroller(false);
        files_scroll.setAutohidesScrollers(true);
        files_scroll.setDocumentView(Some(&files_table));
        content_root.addSubview(&files_scroll);

        let file_count =
            NSTextField::labelWithString(&NSString::from_str("No commit selected"), mtm);
        file_count.setFrame(NSRect::new(
            NSPoint::new(12.0, (content_bounds.size.height - 139.0).max(0.0)),
            NSSize::new(256.0, 20.0),
        ));
        file_count.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        file_count.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        file_count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        file_count.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        content_root.addSubview(&file_count);

        let diff = DiffMetalView::new(
            NSRect::new(
                NSPoint::new(281.0, 0.0),
                NSSize::new(
                    (content_bounds.size.width - 281.0).max(1.0),
                    (content_bounds.size.height - 126.0).max(1.0),
                ),
            ),
            self.ivars().font_sizes.get().diff,
            mtm,
        );
        diff.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        diff.setHidden(true);
        content_root.addSubview(&diff);
        let binary_preview = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(281.0, 0.0),
                NSSize::new(
                    (content_bounds.size.width - 281.0).max(1.0),
                    (content_bounds.size.height - 126.0).max(1.0),
                ),
            ),
        );
        binary_preview.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        binary_preview.setHidden(true);
        content_root.addSubview(&binary_preview);
        let empty = NSTextField::labelWithString(&NSString::from_str("Select a commit"), mtm);
        empty.setFrame(NSRect::new(
            NSPoint::new(281.0, content_bounds.size.height / 2.0 - 18.0),
            NSSize::new((content_bounds.size.width - 281.0).max(1.0), 36.0),
        ));
        empty.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        empty.setAlignment(NSTextAlignment::Center);
        empty.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        content_root.addSubview(&empty);

        HistoryUi {
            sidebar_root,
            search,
            table,
            menu: history_menu,
            scroll,
            loading_spinner,
            status,
            content_root,
            title,
            avatar,
            metadata,
            added,
            deleted,
            comment,
            copy_hash,
            open_remote,
            files_table,
            files_scroll,
            file_count,
            diff,
            binary_preview,
            binary_font_registrations: RefCell::new(Vec::new()),
            preview_cache: RefCell::new(VecDeque::new()),
            empty,
            commits: RefCell::new(Vec::new()),
            files: RefCell::new(Vec::new()),
            query: RefCell::new(String::new()),
            cursor: RefCell::new(None),
            selected_hash: RefCell::new(None),
            selected_commit: RefCell::new(None),
            selected_parent_hash: RefCell::new(None),
            parent_loaded: Cell::new(false),
            pending_checkout_parent: Cell::new(false),
            pending_amend: Cell::new(false),
            detail_loading: Cell::new(false),
            selected_file: RefCell::new(None),
            loaded_diff_path: RefCell::new(None),
            loaded_binary_path: RefCell::new(None),
            avatar_source: RefCell::new(None),
            has_more: Cell::new(false),
            loading: Cell::new(false),
            pending_search: Cell::new(false),
            generation: Cell::new(0),
            detail_request_id: Cell::new(0),
            comparison_request_id: Cell::new(0),
            action_in_progress: Cell::new(false),
        }
    }

}
