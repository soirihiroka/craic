impl AppDelegate {
    fn make_containers_ui(&self, sidebar_bounds: NSRect, content_bounds: NSRect) -> ContainersUi {
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
        search.setPlaceholderString(Some(&NSString::from_str("Search containers")));
        search.setSendsSearchStringImmediately(true);
        search.setHidden(true);
        unsafe {
            search.setTarget(Some(self));
            search.setAction(Some(sel!(filterContainers:)));
        }
        sidebar_root.addSubview(&search);

        let table = ContainersTableView::new(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new((sidebar_bounds.size.width - 16.0).max(1.0), 1.0),
            ),
            mtm,
        );
        table.attach_delegate(self);
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(mtm),
            &NSUserInterfaceItemIdentifier::from_str("containers.inventory"),
        );
        column.setWidth((sidebar_bounds.size.width - 16.0).max(1.0));
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        table.setRowHeight(CONTAINER_ROW_HEIGHT);
        table.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        table.setAllowsEmptySelection(true);
        table.setAllowsMultipleSelection(false);
        table.setUsesAlternatingRowBackgroundColors(false);
        table.setBackgroundColor(&NSColor::clearColor());
        table.setFloatsGroupRows(true);
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let container_menu = NSMenu::new(mtm);
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(8.0, 8.0),
                NSSize::new(
                    (sidebar_bounds.size.width - 16.0).max(1.0),
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
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&table));
        scroll.setHidden(true);
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
            &NSString::from_str("Open a workspace to load containers."),
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
        let title = NSTextField::labelWithString(&NSString::from_str("Containers"), mtm);
        title.setFrame(NSRect::new(
            NSPoint::new(24.0, content_bounds.size.height - 54.0),
            NSSize::new((content_bounds.size.width - 48.0).max(1.0), 28.0),
        ));
        title.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        title.setFont(Some(&NSFont::boldSystemFontOfSize(18.0)));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        content_root.addSubview(&title);
        let subtitle = NSTextField::labelWithString(&NSString::new(), mtm);
        subtitle.setFrame(NSRect::new(
            NSPoint::new(24.0, content_bounds.size.height - 80.0),
            NSSize::new((content_bounds.size.width - 48.0).max(1.0), 20.0),
        ));
        subtitle.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
        subtitle.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        content_root.addSubview(&subtitle);

        let mut action_buttons = Vec::new();
        for (title_text, action, width) in [
            ("Logs", sel!(showContainerLogs:), 58.0),
            ("Inspect", sel!(inspectContainer:), 66.0),
            ("Shell", sel!(attachContainerShell:), 58.0),
            ("Start", sel!(startContainer:), 58.0),
            ("Stop", sel!(stopContainer:), 58.0),
            ("Restart", sel!(restartContainer:), 68.0),
            ("Remove", sel!(removeContainer:), 70.0),
        ] {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title_text),
                    Some(self),
                    Some(action),
                    mtm,
                )
            };
            let x = 24.0
                + action_buttons
                    .iter()
                    .map(|button: &Retained<NSButton>| button.frame().size.width + 8.0)
                    .sum::<f64>();
            button.setFrame(NSRect::new(
                NSPoint::new(x, content_bounds.size.height - 116.0),
                NSSize::new(width, 28.0),
            ));
            button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
            button.setBezelStyle(NSBezelStyle::AccessoryBar);
            button.setControlSize(NSControlSize::Small);
            button.setEnabled(false);
            if title_text == "Remove" {
                button.setHasDestructiveAction(true);
            }
            content_root.addSubview(&button);
            action_buttons.push(button);
        }
        let logs = action_buttons.remove(0);
        let inspect = action_buttons.remove(0);
        let shell = action_buttons.remove(0);
        let start = action_buttons.remove(0);
        let stop = action_buttons.remove(0);
        let restart = action_buttons.remove(0);
        let remove = action_buttons.remove(0);
        for button in [&logs, &shell, &start, &stop, &restart, &remove] {
            button.setHidden(true);
        }
        inspect.setFrameOrigin(NSPoint::new(24.0, content_bounds.size.height - 116.0));

        let empty = NSTextField::wrappingLabelWithString(
            &NSString::from_str("Select a container or Compose project."),
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

        let details_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(
                    (content_bounds.size.width - 40.0).max(1.0),
                    (content_bounds.size.height - 154.0).max(1.0),
                ),
            ),
        );
        details_content.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let details_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(20.0, 20.0),
                NSSize::new(
                    (content_bounds.size.width - 40.0).max(1.0),
                    (content_bounds.size.height - 154.0).max(1.0),
                ),
            ),
        );
        details_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        details_scroll.setBorderType(NSBorderType::NoBorder);
        details_scroll.setDrawsBackground(false);
        details_scroll.setHasVerticalScroller(true);
        details_scroll.setAutohidesScrollers(true);
        details_scroll.setDocumentView(Some(&details_content));
        details_scroll.setHidden(true);
        content_root.addSubview(&details_scroll);
        inspect.removeFromSuperview();
        inspect.setHidden(true);

        let inspect_code = CodeMetalView::new(
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(
                    content_bounds.size.width.max(1.0),
                    (content_bounds.size.height - 88.0).max(1.0),
                ),
            ),
            self.ivars().font_sizes.get().editor,
            mtm,
        );
        inspect_code.attach_delegate(self);
        inspect_code.setAccessibilityLabel(Some(&NSString::from_str("Docker inspect JSON")));
        inspect_code.setHidden(true);
        content_root.addSubview(&inspect_code);

        ContainersUi {
            sidebar_root,
            search,
            table,
            scroll,
            status,
            spinner,
            content_root,
            title,
            subtitle,
            empty,
            details_scroll,
            details_content,
            inspect_code,
            logs,
            inspect,
            shell,
            start,
            stop,
            restart,
            remove,
            menu: container_menu,
            rows: RefCell::new(Vec::new()),
            expanded_groups: RefCell::new(HashSet::new()),
            selected_id: RefCell::new(None),
            selected_group_key: RefCell::new(None),
            query: RefCell::new(String::new()),
            generation: Cell::new(0),
            loading: Cell::new(false),
            dirty: Cell::new(true),
            detail_request_id: Cell::new(0),
            action_request_id: Cell::new(0),
            action_in_progress: Cell::new(false),
            context_selection: Cell::new(false),
        }
    }

}
