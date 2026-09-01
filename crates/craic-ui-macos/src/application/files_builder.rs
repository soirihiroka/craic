impl AppDelegate {
    fn make_files_ui(&self, sidebar_bounds: NSRect, content_bounds: NSRect) -> FilesUi {
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
                NSPoint::new(10.0, sidebar_bounds.size.height - 42.0),
                NSSize::new((sidebar_bounds.size.width - 20.0).max(1.0), 30.0),
            ),
        );
        search.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        search.setPlaceholderString(Some(&NSString::from_str("Search workspace")));
        search.setSendsSearchStringImmediately(true);
        search.setHidden(true);
        unsafe {
            search.setTarget(Some(self));
            search.setAction(Some(sel!(filterFiles:)));
        }
        sidebar_root.addSubview(&search);

        let table = FilesTableView::new(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new((sidebar_bounds.size.width - 16.0).max(1.0), 1.0),
            ),
            mtm,
        );
        table.attach_delegate(self);
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(mtm),
            &NSUserInterfaceItemIdentifier::from_str("files.workspace"),
        );
        column.setWidth((sidebar_bounds.size.width - 16.0).max(1.0));
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        table.setStyle(NSTableViewStyle::FullWidth);
        table.setRowHeight(FILE_ROW_HEIGHT);
        table.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        table.setAllowsEmptySelection(true);
        table.setAllowsMultipleSelection(false);
        table.setUsesAlternatingRowBackgroundColors(false);
        table.setBackgroundColor(&NSColor::clearColor());
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
            table.setTarget(Some(self));
            table.setDoubleAction(Some(sel!(activateWorkspaceSelection:)));
        }
        let file_url_type = unsafe { NSPasteboardTypeFileURL };
        let drag_type = workspace_file_drag_type();
        table.registerForDraggedTypes(&NSArray::from_slice(&[file_url_type, &*drag_type]));
        table.setDraggingSourceOperationMask_forLocal(
            NSDragOperation::Copy | NSDragOperation::Move,
            true,
        );
        let file_menu = NSMenu::new(mtm);
        file_menu.setAutoenablesItems(false);
        file_menu.setDelegate(Some(ProtocolObject::from_ref(self)));
        // SAFETY: The retained menu targets this main-thread delegate for the table's lifetime.
        unsafe { table.setMenu(Some(&file_menu)) };

        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 8.0),
                NSSize::new(
                    sidebar_bounds.size.width.max(1.0),
                    (sidebar_bounds.size.height - 58.0).max(1.0),
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
        sidebar_root.addSubview(&scroll);

        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(
                NSPoint::new(
                    sidebar_bounds.size.width / 2.0 - 10.0,
                    sidebar_bounds.size.height / 2.0,
                ),
                NSSize::new(20.0, 20.0),
            ),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setIndeterminate(true);
        spinner.setDisplayedWhenStopped(false);
        spinner.setHidden(true);
        spinner.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewMaxXMargin
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        sidebar_root.addSubview(&spinner);

        let status = NSTextField::wrappingLabelWithString(
            &NSString::from_str("Open a workspace to browse files."),
            mtm,
        );
        status.setFrame(NSRect::new(
            NSPoint::new(24.0, sidebar_bounds.size.height / 2.0 - 42.0),
            NSSize::new((sidebar_bounds.size.width - 48.0).max(1.0), 40.0),
        ));
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        status.setAlignment(NSTextAlignment::Center);
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setMaximumNumberOfLines(2);
        sidebar_root.addSubview(&status);

        let content_root = NSView::initWithFrame(NSView::alloc(mtm), content_bounds);
        content_root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content_root.setHidden(true);
        let title = NSTextField::labelWithString(&NSString::from_str("Select a file"), mtm);
        title.setFrame(NSRect::new(
            NSPoint::new(24.0, content_bounds.size.height - 54.0),
            NSSize::new((content_bounds.size.width - 48.0).max(1.0), 28.0),
        ));
        title.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        title.setFont(Some(&NSFont::boldSystemFontOfSize(18.0)));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        title.setSelectable(true);
        content_root.addSubview(&title);

        let metadata = NSTextField::labelWithString(&NSString::new(), mtm);
        metadata.setFrame(NSRect::new(
            NSPoint::new(24.0, content_bounds.size.height - 80.0),
            NSSize::new((content_bounds.size.width - 48.0).max(1.0), 20.0),
        ));
        metadata.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        metadata.setTextColor(Some(&NSColor::secondaryLabelColor()));
        metadata.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        metadata.setSelectable(true);
        content_root.addSubview(&metadata);

        let empty = NSTextField::wrappingLabelWithString(
            &NSString::from_str("Select a file or folder from the workspace tree."),
            mtm,
        );
        empty.setFrame(NSRect::new(
            NSPoint::new(40.0, content_bounds.size.height / 2.0 - 30.0),
            NSSize::new((content_bounds.size.width - 80.0).max(1.0), 60.0),
        ));
        empty.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        empty.setAlignment(NSTextAlignment::Center);
        empty.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        empty.setMaximumNumberOfLines(3);
        content_root.addSubview(&empty);

        let preview_frame = NSRect::new(
            NSPoint::new(0.0, 20.0),
            NSSize::new(
                content_bounds.size.width.max(1.0),
                (content_bounds.size.height - 112.0).max(1.0),
            ),
        );
        let preview_text = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(preview_frame.size.width, preview_frame.size.height),
            ),
        );
        preview_text.setEditable(false);
        preview_text.setSelectable(true);
        preview_text.setRichText(false);
        preview_text.setDrawsBackground(false);
        preview_text.setAutomaticQuoteSubstitutionEnabled(false);
        preview_text.setAutomaticDashSubstitutionEnabled(false);
        preview_text.setUsesFindBar(true);
        preview_text.setIncrementalSearchingEnabled(true);
        preview_text.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            self.ivars().font_sizes.get().editor,
            0.0,
        )));
        preview_text.setTextContainerInset(NSSize::new(10.0, 10.0));
        preview_text.setDelegate(Some(ProtocolObject::from_ref(self)));
        let preview_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), preview_frame);
        preview_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        preview_scroll.setBorderType(NSBorderType::NoBorder);
        preview_scroll.setDrawsBackground(false);
        preview_scroll.setHasVerticalScroller(true);
        preview_scroll.setHasHorizontalScroller(true);
        preview_scroll.setAutohidesScrollers(true);
        preview_scroll.setDocumentView(Some(&preview_text));
        preview_scroll.setHidden(true);
        content_root.addSubview(&preview_scroll);

        let preview_code =
            CodeMetalView::new(preview_frame, self.ivars().font_sizes.get().editor, mtm);
        preview_code.attach_delegate(self);
        preview_code.setHidden(true);
        content_root.addSubview(&preview_code);

        let editor_search_panel = NSGlassEffectView::initWithFrame(
            NSGlassEffectView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(400.0, 46.0)),
        );
        editor_search_panel.setStyle(NSGlassEffectViewStyle::Regular);
        editor_search_panel.setCornerRadius(23.0);
        let editor_search_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(400.0, 46.0)),
        );
        let editor_search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            NSRect::new(NSPoint::new(12.0, 8.0), NSSize::new(190.0, 30.0)),
        );
        editor_search.setPlaceholderString(Some(&NSString::from_str("Search file")));
        editor_search.setSendsSearchStringImmediately(true);
        unsafe {
            editor_search.setTarget(Some(self));
            editor_search.setAction(Some(sel!(filterEditor:)));
        }
        editor_search_content.addSubview(&editor_search);
        let editor_search_status = NSTextField::labelWithString(&NSString::new(), mtm);
        editor_search_status.setFrame(NSRect::new(
            NSPoint::new(208.0, 13.0),
            NSSize::new(60.0, 20.0),
        ));
        editor_search_status.setAlignment(NSTextAlignment::Center);
        editor_search_status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        editor_search_status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        editor_search_content.addSubview(&editor_search_status);
        for (x, symbol, tooltip, action) in [
            (
                272.0,
                "chevron.up",
                "Previous match",
                sel!(previousEditorMatch:),
            ),
            (308.0, "chevron.down", "Next match", sel!(nextEditorMatch:)),
            (356.0, "xmark", "Close search", sel!(closeEditorSearch:)),
        ] {
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(tooltip)),
            )
            .expect("macOS provides editor search SF Symbols");
            let button = unsafe {
                NSButton::buttonWithImage_target_action(&image, Some(self), Some(action), mtm)
            };
            button.setFrame(NSRect::new(NSPoint::new(x, 7.0), NSSize::new(32.0, 32.0)));
            button.setBezelStyle(NSBezelStyle::Circular);
            button.setBordered(true);
            button.setToolTip(Some(&NSString::from_str(tooltip)));
            editor_search_content.addSubview(&button);
        }
        editor_search_panel.setContentView(Some(&editor_search_content));
        editor_search_panel.setHidden(true);
        content_root.addSubview(&editor_search_panel);

        let web_configuration =
            unsafe { WKWebViewConfiguration::init(WKWebViewConfiguration::alloc(mtm)) };
        // SAFETY: The configuration and its default preferences remain confined to AppKit's main
        // thread; untrusted preview documents do not need content JavaScript.
        unsafe {
            web_configuration
                .defaultWebpagePreferences()
                .setAllowsContentJavaScript(false)
        };
        let preview_web_content = unsafe { web_configuration.userContentController() };
        let source_map_script = unsafe {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(craic_ui_preview::markdown_preview_web::SOURCE_MAP_SCRIPT),
                WKUserScriptInjectionTime::AtDocumentEnd,
                true,
            )
        };
        // SAFETY: Only the bundled source-map bridge is injected. Page-authored JavaScript stays
        // disabled, and the main-thread delegate validates both the handler name and web view.
        unsafe {
            preview_web_content.addUserScript(&source_map_script);
        }
        let preview_web = unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                preview_frame,
                &web_configuration,
            )
        };
        // SAFETY: AppDelegate outlives the retained web view and WebKit holds this delegate weakly.
        unsafe {
            preview_web.setNavigationDelegate(Some(ProtocolObject::from_ref(self)));
        }
        preview_web.setHidden(true);
        content_root.addSubview(&preview_web);

        let preview_divider = NSBox::initWithFrame(NSBox::alloc(mtm), NSRect::ZERO);
        preview_divider.setBoxType(NSBoxType::Separator);
        preview_divider.setHidden(true);
        content_root.addSubview(&preview_divider);

        let preview_image = NativeImagePreview::new(preview_frame, mtm);
        preview_image.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        preview_image.setHidden(true);
        content_root.addSubview(&preview_image);

        // SAFETY: PDFView is an AppKit NSView subclass initialized and retained on the main thread.
        let preview_pdf = unsafe { PDFView::initWithFrame(PDFView::alloc(mtm), preview_frame) };
        unsafe {
            preview_pdf.setDisplayMode(PDFDisplayMode::SinglePageContinuous);
            preview_pdf.setDisplayDirection(PDFDisplayDirection::Vertical);
            preview_pdf.setDisplaysPageBreaks(true);
            preview_pdf.setAutoScales(true);
        }
        preview_pdf.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        preview_pdf.setHidden(true);
        content_root.addSubview(&preview_pdf);

        let preview_table = NSTableView::initWithFrame(
            NSTableView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(preview_frame.size.width, preview_frame.size.height),
            ),
        );
        let preview_table_header = NSTableHeaderView::initWithFrame(
            NSTableHeaderView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(preview_frame.size.width, 24.0),
            ),
        );
        preview_table.setHeaderView(Some(&preview_table_header));
        preview_table.setRowHeight(26.0);
        preview_table.setIntercellSpacing(NSSize::new(1.0, 1.0));
        preview_table.setAllowsMultipleSelection(true);
        preview_table.setAllowsEmptySelection(true);
        preview_table
            .setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::NoColumnAutoresizing);
        unsafe {
            preview_table.setDataSource(Some(ProtocolObject::from_ref(self)));
            preview_table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let preview_table_scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), preview_frame);
        preview_table_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        preview_table_scroll.setBorderType(NSBorderType::BezelBorder);
        preview_table_scroll.setDrawsBackground(true);
        preview_table_scroll.setHasVerticalScroller(true);
        preview_table_scroll.setHasHorizontalScroller(true);
        preview_table_scroll.setAutohidesScrollers(true);
        preview_table_scroll.setDocumentView(Some(&preview_table));
        preview_table_scroll.setHidden(true);
        content_root.addSubview(&preview_table_scroll);

        let sqlite_controls = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        sqlite_controls.setHidden(true);
        let sqlite_table_selector =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        sqlite_table_selector.setToolTip(Some(&NSString::from_str("SQLite table or view")));
        unsafe {
            sqlite_table_selector.setTarget(Some(self));
            sqlite_table_selector.setAction(Some(sel!(selectSqliteTable:)));
        }
        sqlite_controls.addSubview(&sqlite_table_selector);

        let sqlite_column_selector =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        sqlite_column_selector.setToolTip(Some(&NSString::from_str("Filter column")));
        unsafe {
            sqlite_column_selector.setTarget(Some(self));
            sqlite_column_selector.setAction(Some(sel!(selectSqliteFilterColumn:)));
        }
        sqlite_controls.addSubview(&sqlite_column_selector);

        let sqlite_filter = NSSearchField::initWithFrame(NSSearchField::alloc(mtm), NSRect::ZERO);
        sqlite_filter.setPlaceholderString(Some(&NSString::from_str("Filter rows")));
        sqlite_filter.setSendsSearchStringImmediately(true);
        unsafe {
            sqlite_filter.setTarget(Some(self));
            sqlite_filter.setAction(Some(sel!(filterSqliteRows:)));
        }
        sqlite_controls.addSubview(&sqlite_filter);

        let sqlite_status = NSTextField::labelWithString(&NSString::from_str("Page –"), mtm);
        sqlite_status.setAlignment(NSTextAlignment::Right);
        sqlite_status.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
        sqlite_status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        sqlite_status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        sqlite_controls.addSubview(&sqlite_status);

        let sqlite_previous = NSButton::new(mtm);
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("chevron.left"),
            Some(&NSString::from_str("Previous SQLite page")),
        ) {
            sqlite_previous.setImage(Some(&image));
        }
        sqlite_previous.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        sqlite_previous.setToolTip(Some(&NSString::from_str("Previous page")));
        sqlite_previous.setEnabled(false);
        unsafe {
            sqlite_previous.setTarget(Some(self));
            sqlite_previous.setAction(Some(sel!(previousSqlitePage:)));
        }
        sqlite_controls.addSubview(&sqlite_previous);

        let sqlite_next = NSButton::new(mtm);
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("chevron.right"),
            Some(&NSString::from_str("Next SQLite page")),
        ) {
            sqlite_next.setImage(Some(&image));
        }
        sqlite_next.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        sqlite_next.setToolTip(Some(&NSString::from_str("Next page")));
        sqlite_next.setEnabled(false);
        unsafe {
            sqlite_next.setTarget(Some(self));
            sqlite_next.setAction(Some(sel!(nextSqlitePage:)));
        }
        sqlite_controls.addSubview(&sqlite_next);

        let sqlite_reload = NSButton::new(mtm);
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("arrow.clockwise"),
            Some(&NSString::from_str("Reload SQLite database")),
        ) {
            sqlite_reload.setImage(Some(&image));
        }
        sqlite_reload.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        sqlite_reload.setToolTip(Some(&NSString::from_str("Reload database")));
        unsafe {
            sqlite_reload.setTarget(Some(self));
            sqlite_reload.setAction(Some(sel!(reloadSqlitePreview:)));
        }
        sqlite_controls.addSubview(&sqlite_reload);
        content_root.addSubview(&sqlite_controls);

        let preview_spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(
                NSPoint::new(
                    content_bounds.size.width / 2.0 - 10.0,
                    content_bounds.size.height / 2.0 + 24.0,
                ),
                NSSize::new(20.0, 20.0),
            ),
        );
        preview_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        preview_spinner.setControlSize(NSControlSize::Small);
        preview_spinner.setIndeterminate(true);
        preview_spinner.setDisplayedWhenStopped(false);
        preview_spinner.setHidden(true);
        preview_spinner.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewMaxXMargin
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        content_root.addSubview(&preview_spinner);

        FilesUi {
            sidebar_root,
            search,
            table,
            menu: file_menu,
            scroll,
            status,
            spinner,
            content_root,
            title,
            metadata,
            metadata_base: RefCell::new(String::new()),
            empty,
            preview_scroll,
            preview_text,
            preview_code,
            editor_search_panel,
            editor_search,
            editor_search_status,
            editor_search_visible: Cell::new(false),
            preview_web,
            preview_web_content,
            preview_divider,
            preview_image,
            preview_pdf,
            preview_table_scroll,
            preview_table,
            preview_table_columns: RefCell::new(Vec::new()),
            preview_table_rows: RefCell::new(Vec::new()),
            font_registration: RefCell::new(None),
            sqlite_controls,
            sqlite_table_selector,
            sqlite_column_selector,
            sqlite_filter,
            sqlite_previous,
            sqlite_next,
            sqlite_reload,
            sqlite_status,
            sqlite_state: RefCell::new(None),
            sqlite_generation: Cell::new(0),
            preview_spinner,
            rows: RefCell::new(Vec::new()),
            expanded: RefCell::new(HashSet::new()),
            selected_path: RefCell::new(None),
            query: RefCell::new(String::new()),
            generation: Cell::new(0),
            loading: Cell::new(false),
            mutation_in_progress: Cell::new(false),
            dirty: Cell::new(false),
            preview_request_id: Cell::new(0),
            drop_hover_generation: Cell::new(0),
            drop_hover_path: RefCell::new(None),
            loaded_text_path: RefCell::new(None),
            loaded_text_signature: RefCell::new(None),
            text_buffer: RefCell::new(String::new()),
            text_selection: Cell::new(NSRange::new(0, 0)),
            text_editable: Cell::new(false),
            pending_text_selection: RefCell::new(None),
            text_edit_generation: Cell::new(0),
            text_dirty: Cell::new(false),
            text_save_in_progress: Cell::new(false),
            preview_web_mode: Cell::new(NativeWebPreviewMode::Hidden),
            markdown_editor_source_offset: Cell::new(None),
            suppress_text_change: Cell::new(false),
        }
    }

}
