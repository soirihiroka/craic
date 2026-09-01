impl AppDelegate {
    fn refresh_changed_file_results(&self) {
        let Some(list) = self.ivars().changes_list.get() else {
            return;
        };
        let scroll = self.ivars().changes_scroll.get();
        let previous_origin = scroll
            .map(|scroll| scroll.contentView().bounds().origin)
            .unwrap_or_else(|| NSPoint::new(0.0, 0.0));
        let subviews = list.subviews();
        let first_population = subviews.count() == 0;
        for index in 0..subviews.count() {
            subviews.objectAtIndex(index).removeFromSuperview();
        }
        let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() else {
            return;
        };
        let query = self.ivars().changes_filter_query.borrow().clone();
        let visible_files = snapshot
            .changed_files
            .iter()
            .enumerate()
            .filter(|(_, file)| changed_file_matches_query(&file.path, &file.status, &query))
            .collect::<Vec<_>>();
        let viewport_height = self
            .ivars()
            .changes_scroll
            .get()
            .map(|scroll| scroll.contentView().bounds().size.height)
            .unwrap_or(480.0);
        let content_width = self
            .ivars()
            .changes_scroll
            .get()
            .map(|scroll| scroll.contentView().bounds().size.width)
            .unwrap_or(SIDEBAR_WIDTH - 20.0)
            .max(1.0);
        let content_height =
            (visible_files.len() as f64 * CHANGED_FILE_ROW_HEIGHT).max(viewport_height);
        list.setFrameSize(NSSize::new(content_width, content_height));

        let selected = self.ivars().selected_change_path.borrow().clone();
        let checked = self.ivars().checked_change_paths.borrow();
        for (visible_index, (source_index, file)) in visible_files.into_iter().enumerate() {
            let y = content_height - (visible_index as f64 + 1.0) * CHANGED_FILE_ROW_HEIGHT;
            let is_selected = selected.as_deref() == Some(file.path.as_str());
            let container = NSView::initWithFrame(
                NSView::alloc(self.mtm()),
                NSRect::new(
                    NSPoint::new(0.0, y),
                    NSSize::new(content_width, CHANGED_FILE_ROW_HEIGHT),
                ),
            );
            container.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            let highlight = NSBox::initWithFrame(
                NSBox::alloc(self.mtm()),
                NSRect::new(
                    NSPoint::new(CHANGED_FILE_ROW_INSET, 1.0),
                    NSSize::new(
                        content_width - CHANGED_FILE_ROW_INSET * 2.0,
                        CHANGED_FILE_ROW_HEIGHT - 2.0,
                    ),
                ),
            );
            highlight.setBoxType(NSBoxType::Custom);
            highlight.setBorderWidth(0.0);
            highlight.setFillColor(&NSColor::unemphasizedSelectedContentBackgroundColor());
            highlight.setCornerRadius(6.0);
            highlight.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            highlight.setHidden(!is_selected);
            container.addSubview(&highlight);
            // SAFETY: The target implements toggleChangedFile: with an NSButton sender.
            let check = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::new(),
                    Some(self),
                    Some(sel!(toggleChangedFile:)),
                    self.mtm(),
                )
            };
            check.setTag(source_index as isize);
            check.setFrame(NSRect::new(NSPoint::new(8.0, 4.0), NSSize::new(26.0, 28.0)));
            check.setState(if checked.contains(&file.path) {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            check.setToolTip(Some(&NSString::from_str(if checked.contains(&file.path) {
                "Include in commit"
            } else {
                "Exclude from commit"
            })));
            container.addSubview(&check);

            let (symbol, status_description) = changed_file_symbol(&file.status);
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(status_description)),
            )
            .expect("macOS provides changed-file SF Symbols");
            // SAFETY: The target implements activateChangedFile: with an NSButton sender.
            let row = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&file.path),
                    Some(self),
                    Some(sel!(activateChangedFile:)),
                    self.mtm(),
                )
            };
            row.setTag(source_index as isize);
            row.setFrame(NSRect::new(
                NSPoint::new(42.0, 1.0),
                NSSize::new(content_width - 76.0, CHANGED_FILE_ROW_HEIGHT - 2.0),
            ));
            row.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            row.setButtonType(NSButtonType::PushOnPushOff);
            row.setState(if is_selected {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            // Selection is drawn independently of the borderless control so
            // AppKit never changes the image/title metrics between states.
            row.setBordered(false);
            row.setAlignment(NSTextAlignment::Left);
            row.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
            row.setFont(Some(&NSFont::systemFontOfSize(13.0)));
            row.setToolTip(Some(&NSString::from_str(&format!(
                "{} — {}",
                file.path, status_description
            ))));
            container.addSubview(&row);
            let status = NSImageView::imageViewWithImage(&image, self.mtm());
            status.setFrame(NSRect::new(
                NSPoint::new(content_width - 28.0, 10.0),
                NSSize::new(16.0, 16.0),
            ));
            status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            let status_color = if is_selected {
                NSColor::selectedControlTextColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            status.setContentTintColor(Some(&status_color));
            container.addSubview(&status);
            let file_menu = NSMenu::new(self.mtm());
            for (title, action, symbol) in [
                (
                    "Open With Default Program",
                    sel!(openChangedFile:),
                    "arrow.up.forward.app",
                ),
                (
                    "Open in Visual Studio Code",
                    sel!(openChangedFileInCode:),
                    "chevron.left.forwardslash.chevron.right",
                ),
                (
                    "Reveal in Finder",
                    sel!(revealChangedFile:),
                    "magnifyingglass",
                ),
                ("Show in Files", sel!(showChangedFileInFiles:), "folder"),
            ] {
                let item = unsafe {
                    file_menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::new(),
                    )
                };
                item.setTag(source_index as isize);
                unsafe { item.setTarget(Some(self)) };
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(title)),
                ) {
                    item.setImage(Some(&image));
                }
            }

            let ignore_options = gitignore::options_for_path(&file.path, IgnoreTargetKind::File);
            if ignore_options.direct.is_some()
                || !ignore_options.folders.is_empty()
                || ignore_options.extension.is_some()
            {
                file_menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            }
            if let Some(option) = ignore_options.direct {
                let item = unsafe {
                    file_menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str(&option.label),
                        Some(sel!(addChangedIgnorePattern:)),
                        &NSString::new(),
                    )
                };
                unsafe {
                    item.setTarget(Some(self));
                    item.setRepresentedObject(Some(&NSString::from_str(&option.pattern)));
                }
            }
            if !ignore_options.folders.is_empty() {
                let folders = NSMenu::new(self.mtm());
                for option in ignore_options.folders {
                    let item = unsafe {
                        folders.addItemWithTitle_action_keyEquivalent(
                            &NSString::from_str(&option.label),
                            Some(sel!(addChangedIgnorePattern:)),
                            &NSString::new(),
                        )
                    };
                    unsafe {
                        item.setTarget(Some(self));
                        item.setRepresentedObject(Some(&NSString::from_str(&option.pattern)));
                    }
                }
                let folder_item = unsafe {
                    file_menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str("Ignore Folder (Add to .gitignore)"),
                        None,
                        &NSString::new(),
                    )
                };
                folder_item.setSubmenu(Some(&folders));
            }
            if let Some(option) = ignore_options.extension {
                let item = unsafe {
                    file_menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str(&option.label),
                        Some(sel!(addChangedIgnorePattern:)),
                        &NSString::new(),
                    )
                };
                unsafe {
                    item.setTarget(Some(self));
                    item.setRepresentedObject(Some(&NSString::from_str(&option.pattern)));
                }
            }

            file_menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            for (title, action, symbol) in [
                (
                    "Copy Relative Path",
                    sel!(copyChangedRelativePath:),
                    "doc.on.doc",
                ),
                (
                    "Copy Absolute Path",
                    sel!(copyChangedAbsolutePath:),
                    "doc.on.doc.fill",
                ),
            ] {
                let item = unsafe {
                    file_menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::new(),
                    )
                };
                item.setTag(source_index as isize);
                unsafe { item.setTarget(Some(self)) };
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(title)),
                ) {
                    item.setImage(Some(&image));
                }
            }
            file_menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            let discard = unsafe {
                file_menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str("Discard Changes…"),
                    Some(sel!(confirmDiscardChangedFile:)),
                    &NSString::new(),
                )
            };
            discard.setTag(source_index as isize);
            unsafe { discard.setTarget(Some(self)) };
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("trash"),
                Some(&NSString::from_str("Discard Changes")),
            ) {
                discard.setImage(Some(&image));
            }
            unsafe {
                container.setMenu(Some(&file_menu));
                check.setMenu(Some(&file_menu));
                row.setMenu(Some(&file_menu));
                status.setMenu(Some(&file_menu));
            }
            list.addSubview(&container);
        }
        if let Some(scroll) = scroll {
            let clip = scroll.contentView();
            let current_bounds = clip.bounds();
            let proposed_origin = NSPoint::new(
                previous_origin.x.max(0.0),
                if first_population {
                    content_height
                } else {
                    previous_origin.y
                },
            );
            // Ask NSClipView to constrain the preserved origin so AppKit accounts for its
            // content inset and the newly rebuilt document geometry. A hand-computed clamp
            // either drops the native top inset or strands rows after the list shrinks.
            let constrained =
                clip.constrainBoundsRect(NSRect::new(proposed_origin, current_bounds.size));
            clip.scrollToPoint(constrained.origin);
            scroll.reflectScrolledClipView(&clip);
        }
        self.refresh_selection_header();
    }

    fn refresh_selection_header(&self) {
        let query = self.ivars().changes_filter_query.borrow();
        let snapshot = self.ivars().repository_snapshot.borrow();
        let checked_paths = self.ivars().checked_change_paths.borrow();
        let (total, checked) = snapshot.as_ref().map_or((0, 0), |snapshot| {
            snapshot
                .changed_files
                .iter()
                .filter(|file| changed_file_matches_query(&file.path, &file.status, &query))
                .fold((0, 0), |(total, checked), file| {
                    (
                        total + 1,
                        checked + usize::from(checked_paths.contains(&file.path)),
                    )
                })
        });
        if let Some(check) = self.ivars().select_all_check.get() {
            check.setEnabled(total > 0);
            check.setState(match (checked, total) {
                (0, _) => NSControlStateValueOff,
                (checked, total) if checked == total => NSControlStateValueOn,
                _ => NSControlStateValueMixed,
            });
        }
        if let Some(label) = self.ivars().select_all_label.get() {
            let text = match total {
                1 => "1 changed file".to_string(),
                total => format!("{total} changed files"),
            };
            label.setStringValue(&NSString::from_str(&text));
        }
    }

    fn update_repository_home(&self, snapshot: &RepositorySnapshot, running: bool) {
        let (
            Some(root),
            Some(title),
            Some(subtitle),
            Some(cards),
            Some(git_title),
            Some(git_subtitle),
            Some(action),
        ) = (
            self.ivars().content_home_root.get(),
            self.ivars().content_home_title.get(),
            self.ivars().content_home_subtitle.get(),
            self.ivars().content_home_cards.get(),
            self.ivars().content_home_git_title.get(),
            self.ivars().content_home_git_subtitle.get(),
            self.ivars().content_home_action.get(),
        )
        else {
            return;
        };
        title.setStringValue(&NSString::from_str(&match snapshot.changed_files.len() {
            0 => "No local changes".to_string(),
            1 => "1 changed file".to_string(),
            count => format!("{count} changed files"),
        }));
        subtitle.setStringValue(&NSString::from_str(&format!(
            "{} on {}",
            snapshot.name,
            if snapshot.branch.is_empty() {
                "an unborn branch"
            } else {
                &snapshot.branch
            }
        )));
        let remote_action = craic_vcs::git::repository_remote_suggestion(snapshot);
        let git_card = &cards[0];
        if let Some(initialize_card) = self.ivars().content_home_initialize_card.get() {
            initialize_card.setHidden(true);
        }
        if let Some(suggestion) = remote_action {
            git_title.setStringValue(&NSString::from_str(&suggestion.title));
            git_subtitle.setStringValue(&NSString::from_str(&suggestion.detail));
            action.setTitle(&NSString::from_str(&suggestion.button_label));
            action.setToolTip(Some(&NSString::from_str(&suggestion.button_label)));
            action.setEnabled(!running);
            git_card.setHidden(false);
        } else {
            git_card.setHidden(true);
        }
        let local_workspace = self.active_local_workspace_path().is_ok();
        for button in [
            self.ivars().content_home_editor.get(),
            self.ivars().content_home_terminal.get(),
            self.ivars().content_home_files.get(),
        ]
        .into_iter()
        .flatten()
        {
            button.setEnabled(local_workspace);
        }
        if let Some(remote) = self.ivars().content_home_remote.get() {
            remote.setEnabled(true);
        }
        root.setHidden(false);
        if let Some(empty) = self.ivars().content_empty.get() {
            empty.setHidden(true);
        }
        self.layout_content();
    }

    fn update_repository_initialization_home(&self, name: &str) {
        let (Some(root), Some(title), Some(subtitle), Some(cards), Some(initialize_card)) = (
            self.ivars().content_home_root.get(),
            self.ivars().content_home_title.get(),
            self.ivars().content_home_subtitle.get(),
            self.ivars().content_home_cards.get(),
            self.ivars().content_home_initialize_card.get(),
        ) else {
            return;
        };
        title.setStringValue(&NSString::from_str("Repository not initialized"));
        subtitle.setStringValue(&NSString::from_str(&format!(
            "Initialize Git to track changes in {name}."
        )));
        if let Some(git_card) = cards.first() {
            git_card.setHidden(true);
        }
        initialize_card.setHidden(false);
        if let Some(button) = self.ivars().content_home_initialize.get() {
            let initializing = self.ivars().repository_initialization_in_progress.get();
            button.setTitle(&NSString::from_str(if initializing {
                "Initializing…"
            } else {
                "Initialize"
            }));
            button.setEnabled(!initializing);
        }
        let local_workspace = self.active_local_workspace_path().is_ok();
        for button in [
            self.ivars().content_home_editor.get(),
            self.ivars().content_home_terminal.get(),
            self.ivars().content_home_files.get(),
        ]
        .into_iter()
        .flatten()
        {
            button.setEnabled(local_workspace);
        }
        if let Some(remote) = self.ivars().content_home_remote.get() {
            remote.setEnabled(true);
        }
        root.setHidden(false);
        if let Some(empty) = self.ivars().content_empty.get() {
            empty.setHidden(true);
        }
        self.layout_content();
    }

    fn hide_repository_home(&self) {
        if let Some(root) = self.ivars().content_home_root.get() {
            root.setHidden(true);
        }
    }

    fn request_file_comparison(&self, path: String) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        let request_id = self.ivars().diff_request_id.get().wrapping_add(1);
        self.ivars().diff_request_id.set(request_id);
        self.ivars().diff_loading_request_id.set(Some(request_id));
        self.ivars().loaded_diff_path.borrow_mut().take();
        self.ivars().loaded_image_path.borrow_mut().take();
        let cached = {
            let mut cache = self.ivars().file_preview_cache.borrow_mut();
            cache
                .iter()
                .position(|entry| entry.path == path)
                .and_then(|index| cache.remove(index))
                .map(|entry| {
                    let content = entry.content.clone();
                    cache.push_back(entry);
                    content
                })
        };
        if let Some(cached) = cached {
            log::debug!("native file preview cache hit path={path}");
            match cached {
                CachedFilePreviewContent::Diff(prepared) => {
                    self.apply_file_comparison(&workspace_id, &path, request_id, Ok(prepared))
                }
                CachedFilePreviewContent::Image(comparison) => self.apply_file_bytes_comparison(
                    &workspace_id,
                    &path,
                    request_id,
                    Ok(comparison),
                ),
                CachedFilePreviewContent::Unavailable(message) => {
                    self.ivars().diff_loading_request_id.set(None);
                    self.show_file_comparison_error(&message);
                }
            }
            return;
        }
        self.show_file_comparison_loading(&path);
        let request = if is_changed_binary_preview_path(&path) {
            RepositoryRequest::LoadFileBytesComparison {
                workspace_id,
                handle,
                path,
                request_id,
                cancellation,
            }
        } else {
            RepositoryRequest::LoadFileComparison {
                workspace_id,
                handle,
                path,
                request_id,
                cancellation,
            }
        };
        if let Err(error) = requests.try_send(request) {
            log::warn!("file comparison queue rejected request error={error}");
            if self.ivars().diff_request_id.get() == request_id {
                self.ivars().diff_loading_request_id.set(None);
                self.show_file_comparison_error("Unable to queue diff load");
            }
        }
    }

    fn show_file_comparison_loading(&self, path: &str) {
        log::debug!("native file comparison loading path={path}");
        self.hide_repository_home();
        if let Some(empty) = self.ivars().content_empty.get() {
            empty.setHidden(true);
        }
        if let Some(diff_view) = self.ivars().diff_view.get() {
            diff_view.setHidden(true);
        }
        if let Some(image) = self.ivars().image_preview.get() {
            image.setHidden(true);
        }
        if let Some(binary) = self.ivars().binary_preview.get() {
            binary.setHidden(true);
        }
        if let Some(spinner) = self.ivars().diff_spinner.get() {
            spinner.setHidden(false);
            // SAFETY: The retained progress indicator is animated on the AppKit main thread.
            unsafe { spinner.startAnimation(None) };
        }
    }

    fn apply_file_comparison(
        &self,
        workspace_id: &str,
        path: &str,
        request_id: u64,
        result: Result<PreparedDiff, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || self.ivars().diff_request_id.get() != request_id
            || self.ivars().selected_change_path.borrow().as_deref() != Some(path)
        {
            log::debug!(
                "discarding stale file comparison workspace={} path={} request_id={}",
                workspace_id,
                path,
                request_id
            );
            return;
        }
        if let Some(spinner) = self.ivars().diff_spinner.get() {
            // SAFETY: The retained progress indicator is animated on the AppKit main thread.
            unsafe { spinner.stopAnimation(None) };
            spinner.setHidden(true);
        }
        self.ivars().diff_loading_request_id.set(None);
        match result {
            Ok(prepared) => {
                self.cache_file_preview(path, CachedFilePreviewContent::Diff(prepared.clone()));
                self.ivars()
                    .loaded_diff_path
                    .replace(Some(path.to_string()));
                if let Some(diff_view) = self.ivars().diff_view.get() {
                    diff_view.set_document(
                        path,
                        prepared.fingerprint,
                        prepared.document,
                        prepared.syntax,
                    );
                    diff_view.setHidden(!self.is_active_page("changes"));
                }
                if let Some(binary) = self.ivars().binary_preview.get() {
                    binary.setHidden(true);
                }
                if self.is_active_page("changes")
                    && let Some(empty) = self.ivars().content_empty.get()
                {
                    empty.setHidden(true);
                }
                log::info!(
                    "native Skia Metal file comparison applied workspace={} path={} source_rows={} fingerprint={:016x}",
                    workspace_id,
                    path,
                    prepared.source_rows,
                    prepared.fingerprint
                );
            }
            Err(error) => {
                self.ivars().loaded_diff_path.borrow_mut().take();
                if is_preview_limit_message(&error) {
                    self.cache_file_preview(
                        path,
                        CachedFilePreviewContent::Unavailable(error.clone()),
                    );
                }
                self.show_file_comparison_error(&error);
                log::warn!(
                    "native file comparison failed workspace={} path={}: {}",
                    workspace_id,
                    path,
                    error
                );
            }
        }
    }

    fn apply_file_bytes_comparison(
        &self,
        workspace_id: &str,
        path: &str,
        request_id: u64,
        result: Result<BytesComparison, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || self.ivars().diff_request_id.get() != request_id
            || self.ivars().selected_change_path.borrow().as_deref() != Some(path)
        {
            log::debug!(
                "discarding stale image comparison workspace={} path={} request_id={}",
                workspace_id,
                path,
                request_id
            );
            return;
        }
        if let Some(spinner) = self.ivars().diff_spinner.get() {
            unsafe { spinner.stopAnimation(None) };
            spinner.setHidden(true);
        }
        self.ivars().diff_loading_request_id.set(None);
        let comparison = match result {
            Ok(comparison) => comparison,
            Err(error) => {
                self.ivars().loaded_image_path.borrow_mut().take();
                if is_preview_limit_message(&error) {
                    self.cache_file_preview(
                        path,
                        CachedFilePreviewContent::Unavailable(error.clone()),
                    );
                }
                self.show_file_comparison_error(&error);
                log::warn!(
                    "native binary comparison failed workspace={} path={}: {}",
                    workspace_id,
                    path,
                    error
                );
                return;
            }
        };
        if let Err(error) = self.apply_changed_binary_preview(path, &comparison) {
            self.ivars().loaded_image_path.borrow_mut().take();
            self.show_file_comparison_error(&error);
            log::warn!(
                "native binary preview failed workspace={workspace_id} path={path}: {error}"
            );
            return;
        }
        self.cache_file_preview(path, CachedFilePreviewContent::Image(comparison));
        self.ivars()
            .loaded_image_path
            .replace(Some(path.to_string()));
        if self.is_active_page("changes")
            && let Some(empty) = self.ivars().content_empty.get()
        {
            empty.setHidden(true);
        }
        log::info!("native binary preview applied workspace={workspace_id} path={path}");
    }

    fn apply_changed_binary_preview(
        &self,
        path: &str,
        comparison: &BytesComparison,
    ) -> Result<(), String> {
        let Some(root) = self.ivars().binary_preview.get() else {
            return Err("The native binary comparison view is unavailable.".to_string());
        };
        let registrations = self.populate_native_binary_comparison(root, path, comparison)?;
        self.ivars()
            .binary_font_registrations
            .replace(registrations);
        if let Some(image) = self.ivars().image_preview.get() {
            image.setHidden(true);
        }
        if let Some(diff) = self.ivars().diff_view.get() {
            diff.setHidden(true);
        }
        root.setHidden(!self.is_active_page("changes"));
        Ok(())
    }

    fn populate_native_binary_comparison(
        &self,
        root: &NSView,
        path: &str,
        comparison: &BytesComparison,
    ) -> Result<Vec<NativeFontRegistration>, String> {
        let subviews = root.subviews();
        for index in (0..subviews.count()).rev() {
            subviews.objectAtIndex(index).removeFromSuperview();
        }
        if comparison.before.is_none() && comparison.after.is_none() {
            return Err("No file content is available to preview.".to_string());
        }

        let bounds = root.bounds();
        let title = NSTextField::labelWithString(&NSString::from_str(path), self.mtm());
        title.setFrame(NSRect::new(
            NSPoint::new(18.0, bounds.size.height - 38.0),
            NSSize::new((bounds.size.width - 36.0).max(1.0), 22.0),
        ));
        title.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        title.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        title.setToolTip(Some(&NSString::from_str(path)));
        root.addSubview(&title);

        let preview_frame = NSRect::new(
            NSPoint::new(12.0, 12.0),
            NSSize::new(
                (bounds.size.width - 24.0).max(1.0),
                (bounds.size.height - 56.0).max(1.0),
            ),
        );
        let split = NSSplitView::initWithFrame(NSSplitView::alloc(self.mtm()), preview_frame);
        split.setVertical(true);
        split.setDividerStyle(NSSplitViewDividerStyle::Thin);
        split.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let mut registrations = Vec::new();
        let mut add_pane = |label: &str, bytes: &[u8]| {
            let (pane, registration) = self.make_changed_binary_pane(path, label, bytes);
            if let Some(registration) = registration {
                registrations.push(registration);
            }
            split.addArrangedSubview(&pane);
        };
        match (&comparison.before, &comparison.after) {
            (None, Some(after)) => {
                add_pane("Added", after);
            }
            (Some(before), None) => {
                add_pane("Deleted", before);
            }
            (Some(before), Some(after)) => {
                add_pane("Before", before);
                add_pane("After", after);
            }
            (None, None) => unreachable!(),
        }
        drop(add_pane);
        split.adjustSubviews();
        root.addSubview(&split);
        Ok(registrations)
    }

    fn make_changed_binary_pane(
        &self,
        path: &str,
        label: &str,
        bytes: &[u8],
    ) -> (Retained<NSView>, Option<NativeFontRegistration>) {
        let pane = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 600.0)),
        );
        pane.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let heading = NSTextField::labelWithString(&NSString::from_str(label), self.mtm());
        heading.setFrame(NSRect::new(
            NSPoint::new(8.0, 572.0),
            NSSize::new(404.0, 20.0),
        ));
        heading.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        heading.setAlignment(NSTextAlignment::Center);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
        heading.setTextColor(Some(&NSColor::secondaryLabelColor()));
        pane.addSubview(&heading);
        let frame = NSRect::new(NSPoint::new(8.0, 8.0), NSSize::new(404.0, 556.0));

        let mut font_registration = None;
        let result: Result<Retained<NSView>, String> = if is_image_preview_path(path) {
            NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes))
                .map(|image| {
                    let view = NSImageView::imageViewWithImage(&image, self.mtm());
                    view.setFrame(frame);
                    view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                    view.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                        "{label} image"
                    ))));
                    Retained::into_super(Retained::into_super(view))
                })
                .ok_or_else(|| "macOS could not decode this image.".to_string())
        } else if is_pdf_preview_path(path) {
            let data = NSData::with_bytes(bytes);
            let document = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
                .ok_or_else(|| "macOS could not decode this PDF.".to_string());
            document.map(|document| {
                let view = unsafe { PDFView::initWithFrame(PDFView::alloc(self.mtm()), frame) };
                unsafe {
                    view.setAutoScales(true);
                    view.setDisplayMode(PDFDisplayMode::SinglePageContinuous);
                    view.setDisplayDirection(PDFDisplayDirection::Vertical);
                    view.setDocument(Some(&document));
                }
                Retained::into_super(view)
            })
        } else if let Some(mime) = media_preview_mime(path) {
            let configuration =
                unsafe { WKWebViewConfiguration::init(WKWebViewConfiguration::alloc(self.mtm())) };
            unsafe {
                configuration
                    .defaultWebpagePreferences()
                    .setAllowsContentJavaScript(false)
            };
            let view = unsafe {
                WKWebView::initWithFrame_configuration(
                    WKWebView::alloc(self.mtm()),
                    frame,
                    &configuration,
                )
            };
            unsafe { view.setNavigationDelegate(Some(ProtocolObject::from_ref(self))) };
            let base_url = NSURL::URLWithString(&NSString::from_str("about:blank"))
                .expect("about:blank is a valid URL");
            unsafe {
                view.loadData_MIMEType_characterEncodingName_baseURL(
                    &NSData::with_bytes(bytes),
                    &NSString::from_str(mime),
                    &NSString::from_str("utf-8"),
                    &base_url,
                )
            };
            Ok(Retained::into_super(view))
        } else if is_font_preview_path(path) {
            self.make_changed_font_preview(bytes, frame)
                .map(|(view, registration)| {
                    font_registration = Some(registration);
                    Retained::into_super(view)
                })
        } else {
            Err("A native preview is not available for this binary file.".to_string())
        };

        match result {
            Ok(view) => {
                view.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable
                        | NSAutoresizingMaskOptions::ViewHeightSizable,
                );
                pane.addSubview(&view);
            }
            Err(message) => {
                let error =
                    NSTextField::wrappingLabelWithString(&NSString::from_str(&message), self.mtm());
                error.setFrame(NSRect::new(
                    NSPoint::new(24.0, 260.0),
                    NSSize::new(372.0, 52.0),
                ));
                error.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable
                        | NSAutoresizingMaskOptions::ViewMinYMargin
                        | NSAutoresizingMaskOptions::ViewMaxYMargin,
                );
                error.setAlignment(NSTextAlignment::Center);
                error.setTextColor(Some(&NSColor::secondaryLabelColor()));
                pane.addSubview(&error);
            }
        }
        (pane, font_registration)
    }

    fn make_changed_font_preview(
        &self,
        bytes: &[u8],
        frame: NSRect,
    ) -> Result<(Retained<NSScrollView>, NativeFontRegistration), String> {
        let length = isize::try_from(bytes.len())
            .map_err(|_| "This font is too large for Core Graphics.".to_string())?;
        let data = unsafe { CFData::new(None, bytes.as_ptr(), length) }
            .ok_or_else(|| "macOS could not create font data.".to_string())?;
        let descriptor = unsafe { CTFontManagerCreateFontDescriptorFromData(&data) }
            .ok_or_else(|| "macOS could not decode this font.".to_string())?;
        let descriptors = unsafe { CTFontManagerCreateFontDescriptorsFromData(&data) };
        if descriptors.count() == 0 {
            return Err("macOS could not find a font face in this file.".to_string());
        }
        unsafe {
            CTFontManagerRegisterFontDescriptors(
                &descriptors,
                CTFontManagerScope::Process,
                true,
                None,
            );
        }
        let registration = NativeFontRegistration { descriptors };
        let font = unsafe { CTFont::with_font_descriptor(&descriptor, 12.0, std::ptr::null()) };
        let post_script_name = unsafe { font.post_script_name() }.to_string();
        let display_name = unsafe { font.full_name() }.to_string();
        let mut content = String::new();
        let mut ranges = Vec::new();
        let mut append_line = |text: &str, size: f64| {
            let start = content.encode_utf16().count();
            content.push_str(text);
            let length = content.encode_utf16().count() - start;
            ranges.push((NSRange::new(start, length), size));
            content.push('\n');
        };
        append_line(&display_name, 30.0);
        append_line("abcdefghijklmnopqrstuvwxyz", 26.0);
        append_line("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 26.0);
        append_line("0123456789 .:,;(*!?')", 26.0);
        for size in [12.0, 18.0, 24.0, 36.0, 48.0, 72.0] {
            append_line("Sphinx of black quartz, judge my vow.", size);
        }
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(frame.size.width, 920.0)),
        );
        text.setString(&NSString::from_str(&content));
        text.setEditable(false);
        text.setSelectable(true);
        text.setDrawsBackground(false);
        text.setTextContainerInset(NSSize::new(18.0, 18.0));
        let storage = unsafe { text.textStorage() }
            .ok_or_else(|| "macOS could not create font preview text storage.".to_string())?;
        storage.beginEditing();
        for (range, size) in ranges {
            let Some(font) =
                NSFont::fontWithName_size(&NSString::from_str(&post_script_name), size)
            else {
                storage.endEditing();
                return Err(format!("macOS could not activate {display_name}."));
            };
            unsafe { storage.addAttribute_value_range(NSFontAttributeName, &font, range) };
        }
        storage.endEditing();
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(self.mtm()), frame);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&text));
        Ok((scroll, registration))
    }

    fn clear_changed_binary_preview(&self) {
        if let Some(root) = self.ivars().binary_preview.get() {
            root.setHidden(true);
            let subviews = root.subviews();
            for index in (0..subviews.count()).rev() {
                subviews.objectAtIndex(index).removeFromSuperview();
            }
        }
        self.ivars().binary_font_registrations.borrow_mut().clear();
    }

    fn cache_file_preview(&self, path: &str, content: CachedFilePreviewContent) {
        let mut cache = self.ivars().file_preview_cache.borrow_mut();
        if let Some(index) = cache.iter().position(|entry| entry.path == path) {
            cache.remove(index);
        }
        cache.push_back(CachedFilePreview {
            path: path.to_string(),
            content,
        });
        while cache.len() > FILE_PREVIEW_CACHE_CAPACITY {
            cache.pop_front();
        }
    }

    fn show_file_comparison_error(&self, message: &str) {
        self.hide_repository_home();
        if let Some(diff_view) = self.ivars().diff_view.get() {
            diff_view.setHidden(true);
        }
        if let Some(image) = self.ivars().image_preview.get() {
            image.setHidden(true);
        }
        if let Some(binary) = self.ivars().binary_preview.get() {
            binary.setHidden(true);
        }
        if self.is_active_page("changes")
            && let Some(empty) = self.ivars().content_empty.get()
        {
            empty.setStringValue(&NSString::from_str(message));
            empty.setHidden(false);
        }
    }

    fn is_active_page(&self, page_id: &str) -> bool {
        self.ivars().active_page_id.borrow().as_deref() == Some(page_id)
    }

}
