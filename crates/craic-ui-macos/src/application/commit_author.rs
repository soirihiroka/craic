impl AppDelegate {
    fn show_commit_author_picker(&self, sender: &NSButton) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            return;
        };
        let popover = self.commit_author_picker();
        if popover.isShown() {
            popover.close();
            return;
        }

        self.ivars()
            .author_options
            .replace(github::cached_commit_email_options().unwrap_or_default());
        self.ivars().author_loading.set(true);
        self.ivars().author_error.borrow_mut().take();
        log::debug!(
            "native commit author picker opened cached_options={}",
            self.ivars().author_options.borrow().len()
        );
        popover.showRelativeToRect_ofView_preferredEdge(sender.bounds(), sender, NSRectEdge::MaxX);
        self.refresh_commit_author_results();

        let Some(requests) = self.ivars().repository_requests.get() else {
            self.apply_commit_author_options(
                &workspace_id,
                Err("Repository service is unavailable.".to_string()),
            );
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadCommitAuthors {
            workspace_id: workspace_id.clone(),
            handle,
        }) {
            self.apply_commit_author_options(
                &workspace_id,
                Err(format!("Unable to queue GitHub email loading: {error}")),
            );
        }
    }

    fn commit_author_picker(&self) -> Retained<NSPopover> {
        if let Some(popover) = self.ivars().author_popover.get() {
            return popover.clone();
        }

        let root = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(AUTHOR_PICKER_WIDTH, AUTHOR_PICKER_HEIGHT),
            ),
        );
        let table = NSTableView::initWithFrame(
            NSTableView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(AUTHOR_PICKER_WIDTH - 24.0, AUTHOR_PICKER_HEIGHT - 24.0),
            ),
        );
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(self.mtm()),
            &NSUserInterfaceItemIdentifier::from_str("commit.author"),
        );
        column.setWidth(AUTHOR_PICKER_WIDTH - 24.0);
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        table.setRowHeight(AUTHOR_ROW_HEIGHT);
        table.setIntercellSpacing(NSSize::new(0.0, 2.0));
        table.setStyle(NSTableViewStyle::Inset);
        table.setUsesAlternatingRowBackgroundColors(false);
        table.setAllowsEmptySelection(true);
        table.setAllowsMultipleSelection(false);
        table.setBackgroundColor(&NSColor::clearColor());
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(AUTHOR_PICKER_WIDTH - 24.0, AUTHOR_PICKER_HEIGHT - 24.0),
            ),
        );
        scroll.setFrameOrigin(NSPoint::new(12.0, 12.0));
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setAutomaticallyAdjustsContentInsets(true);
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(false);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&table));
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root.addSubview(&scroll);

        let controller = NSViewController::new(self.mtm());
        controller.setView(&root);
        controller.setPreferredContentSize(NSSize::new(AUTHOR_PICKER_WIDTH, AUTHOR_PICKER_HEIGHT));
        let popover = NSPopover::new(self.mtm());
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(NSSize::new(AUTHOR_PICKER_WIDTH, AUTHOR_PICKER_HEIGHT));
        popover.setContentViewController(Some(&controller));

        self.ivars()
            .author_table
            .set(table)
            .expect("commit author table is initialized once");
        self.ivars()
            .author_popover
            .set(popover.clone())
            .expect("commit author popover is initialized once");
        popover
    }

    fn refresh_commit_author_results(&self) {
        let Some(table) = self.ivars().author_table.get() else {
            return;
        };
        self.ivars().author_selection_suppressed.set(true);
        table.reloadData();
        let current = self
            .ivars()
            .repository_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.user_email.as_deref())
            .and_then(|email| {
                self.ivars()
                    .author_options
                    .borrow()
                    .iter()
                    .position(|option| option.email.eq_ignore_ascii_case(email))
            });
        log::debug!(
            "native commit author picker refreshed options={} loading={} error={} current_row={current:?}",
            self.ivars().author_options.borrow().len(),
            self.ivars().author_loading.get(),
            self.ivars().author_error.borrow().is_some(),
        );
        if let Some(row) = current {
            table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
            table.scrollRowToVisible(row as isize);
        } else {
            // SAFETY: The table and delegate are owned by this AppKit main-thread object.
            unsafe { table.deselectAll(None) };
        }
        table.layoutSubtreeIfNeeded();
        self.ivars().author_selection_suppressed.set(false);
    }

    fn make_commit_author_cell(&self, table: &NSTableView, row: usize) -> Option<Retained<NSView>> {
        let width = table.bounds().size.width.max(AUTHOR_PICKER_WIDTH - 24.0);
        let option = self.ivars().author_options.borrow().get(row).cloned();
        let Some(option) = option else {
            let cell = NSView::initWithFrame(
                NSView::alloc(self.mtm()),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(width, AUTHOR_ROW_HEIGHT),
                ),
            );
            let loading = self.ivars().author_loading.get();
            let error = self.ivars().author_error.borrow().clone();
            if loading {
                let spinner = NSProgressIndicator::initWithFrame(
                    NSProgressIndicator::alloc(self.mtm()),
                    NSRect::new(NSPoint::new(12.0, 18.0), NSSize::new(16.0, 16.0)),
                );
                spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                spinner.setControlSize(NSControlSize::Small);
                spinner.setIndeterminate(true);
                spinner.setDisplayedWhenStopped(false);
                // SAFETY: The table creates and animates this indicator on AppKit's main thread.
                unsafe { spinner.startAnimation(None) };
                cell.addSubview(&spinner);
            }
            let (heading, detail) = if loading {
                ("Loading GitHub accounts…", None)
            } else if let Some(error) = error.as_deref() {
                ("Couldn’t load GitHub accounts", Some(error))
            } else {
                (
                    "No GitHub accounts found",
                    Some("Sign in with the GitHub CLI to add one."),
                )
            };
            let heading = NSTextField::labelWithString(&NSString::from_str(heading), self.mtm());
            heading.setFrame(NSRect::new(
                NSPoint::new(if loading { 38.0 } else { 12.0 }, 26.0),
                NSSize::new(width - if loading { 50.0 } else { 24.0 }, 18.0),
            ));
            heading.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            heading.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            cell.addSubview(&heading);
            if let Some(detail) = detail {
                let subtitle =
                    NSTextField::labelWithString(&NSString::from_str(detail), self.mtm());
                subtitle.setFrame(NSRect::new(
                    NSPoint::new(12.0, 7.0),
                    NSSize::new(width - 24.0, 17.0),
                ));
                subtitle.setFont(Some(&NSFont::systemFontOfSize(10.5)));
                subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
                subtitle.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                subtitle.setToolTip(Some(&NSString::from_str(detail)));
                cell.addSubview(&subtitle);
            }
            return Some(cell);
        };

        let display_name = option.name.trim();
        let primary = if display_name.is_empty() {
            option.email.as_str()
        } else {
            display_name
        };
        let is_current = self
            .ivars()
            .repository_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.user_email.as_deref())
            .is_some_and(|email| email.eq_ignore_ascii_case(&option.email));
        let text_width = width - if is_current { 104.0 } else { 68.0 };
        let avatar_source = option
            .avatar_url
            .as_ref()
            .map(|url| (format!("url:{url}"), AvatarSource::Url(url.to_string())));
        let avatar_image = avatar_source
            .as_ref()
            .and_then(|(key, _)| self.ivars().avatar_images.borrow().get(key).cloned())
            .or_else(|| {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("person.crop.circle.fill"),
                    Some(&NSString::from_str(primary)),
                )
            });
        let cell = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, AUTHOR_ROW_HEIGHT),
            ),
        );
        if let Some(image) = avatar_image {
            let avatar = NSImageView::imageViewWithImage(&image, self.mtm());
            avatar.setFrame(NSRect::new(
                NSPoint::new(12.0, 11.0),
                NSSize::new(30.0, 30.0),
            ));
            avatar.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            avatar.setWantsLayer(true);
            if let Some(layer) = avatar.layer() {
                layer.setCornerRadius(15.0);
                layer.setMasksToBounds(true);
            }
            cell.addSubview(&avatar);
        }

        let title = NSTextField::labelWithString(&NSString::from_str(primary), self.mtm());
        title.setFrame(NSRect::new(
            NSPoint::new(52.0, if display_name.is_empty() { 17.0 } else { 27.0 }),
            NSSize::new(text_width.max(1.0), 18.0),
        ));
        title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        title.setFont(Some(&NSFont::systemFontOfSize(12.5)));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        cell.addSubview(&title);

        if !display_name.is_empty() {
            let subtitle =
                NSTextField::labelWithString(&NSString::from_str(&option.email), self.mtm());
            subtitle.setFrame(NSRect::new(
                NSPoint::new(52.0, 8.0),
                NSSize::new(text_width.max(1.0), 17.0),
            ));
            subtitle.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            subtitle.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
            subtitle.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
            subtitle.setToolTip(Some(&NSString::from_str(&option.email)));
            cell.addSubview(&subtitle);
        }

        if is_current {
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("checkmark.circle.fill"),
                Some(&NSString::from_str("Current commit author")),
            ) {
                let current = NSImageView::imageViewWithImage(&image, self.mtm());
                current.setFrame(NSRect::new(
                    NSPoint::new(width - 34.0, 16.0),
                    NSSize::new(20.0, 20.0),
                ));
                current.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
                current.setContentTintColor(Some(&NSColor::controlAccentColor()));
                current.setToolTip(Some(&NSString::from_str("Current commit author")));
                cell.addSubview(&current);
            }
        }
        cell.setToolTip(Some(&NSString::from_str(&format!(
            "Use {primary} <{}>",
            option.email
        ))));
        if let Some((cache_key, source)) = avatar_source
            && !self.ivars().avatar_images.borrow().contains_key(&cache_key)
        {
            self.request_avatar(cache_key, source);
        }
        Some(cell)
    }

    fn apply_commit_author_options(
        &self,
        workspace_id: &str,
        result: Result<Vec<CommitEmailOption>, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        self.ivars().author_loading.set(false);
        match result {
            Ok(options) => {
                log::debug!(
                    "native commit author options loaded workspace={workspace_id} count={}",
                    options.len()
                );
                self.ivars().author_options.replace(options);
                self.ivars().author_error.borrow_mut().take();
            }
            Err(error) => {
                log::warn!("native commit author options failed: {error}");
                self.ivars().author_error.replace(Some(error));
            }
        }
        self.refresh_commit_author_results();
    }

    fn finish_commit_author(
        &self,
        workspace_id: &str,
        handle: Arc<GitRepoHandle>,
        result: Result<WorkspaceSnapshot, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        match result {
            Ok(snapshot) => {
                if let Some(popover) = self.ivars().author_popover.get() {
                    popover.close();
                }
                self.apply_repository_snapshot(workspace_id, Some(handle), None, Ok(snapshot));
                log::info!("native commit author updated workspace={workspace_id}");
            }
            Err(error) => {
                self.present_path_action_error("Author Selection Failed", &error);
                log::warn!("native commit author update failed workspace={workspace_id}: {error}");
            }
        }
    }

}
