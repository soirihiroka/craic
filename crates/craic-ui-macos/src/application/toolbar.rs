impl AppDelegate {
    fn make_toolbar_item(
        &self,
        identifier: &NSToolbarItemIdentifier,
        will_be_inserted: bool,
    ) -> Option<Retained<NSToolbarItem>> {
        let mtm = self.mtm();
        let identifier_string = identifier.to_string();

        if identifier_string == TOOLBAR_PAGES {
            let mut images = Vec::with_capacity(PAGE_DESCRIPTORS.len());
            let mut labels = Vec::with_capacity(PAGE_DESCRIPTORS.len());
            for page in PAGE_DESCRIPTORS {
                let symbol = match page.id {
                    "changes" => "arrow.triangle.branch",
                    "history" => "clock.arrow.circlepath",
                    "files" => "folder",
                    "containers" => "shippingbox",
                    "agents" => "sparkles",
                    _ => "circle",
                };
                images.push(NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(page.label)),
                )?);
                labels.push(NSString::from_str(page.label));
            }
            let images = NSArray::from_retained_slice(&images);
            let labels = NSArray::from_retained_slice(&labels);
            // This is Apple's documented image-based toolbar-group initializer. Keeping the
            // control as an NSToolbarItemGroup lets NSToolbar own its single glass surface.
            let pages = unsafe {
                NSToolbarItemGroup::groupWithItemIdentifier_images_selectionMode_labels_target_action(
                    identifier,
                    &images,
                    NSToolbarItemGroupSelectionMode::SelectOne,
                    Some(&labels),
                    Some(self),
                    Some(sel!(selectPage:)),
                    mtm,
                )
            };
            pages.setControlRepresentation(NSToolbarItemGroupControlRepresentation::Expanded);
            pages.setVisibilityPriority(NSToolbarItemVisibilityPriorityUser);
            pages.setLabel(&NSString::from_str("Pages"));
            pages.setPaletteLabel(&NSString::from_str("Pages"));
            pages.setSelectedIndex(0);
            for (index, page) in PAGE_DESCRIPTORS.iter().enumerate() {
                pages
                    .subitems()
                    .objectAtIndex(index)
                    .setToolTip(Some(&NSString::from_str(page.label)));
            }
            if will_be_inserted && self.ivars().page_switcher.get().is_none() {
                let _ = self.ivars().page_switcher.set(pages.retain());
                self.restore_changes_page_badge();
            }
            return Some(pages.into_super());
        }

        if identifier_string == TOOLBAR_ADD_ACTION {
            let group = NSToolbarItemGroup::initWithItemIdentifier(
                NSToolbarItemGroup::alloc(mtm),
                identifier,
            );
            group.setLabel(&NSString::from_str("Quick Actions"));
            group.setPaletteLabel(&NSString::from_str("Quick Actions"));
            group.setControlRepresentation(NSToolbarItemGroupControlRepresentation::Automatic);
            if will_be_inserted && self.ivars().quick_action_group.get().is_none() {
                let _ = self.ivars().quick_action_group.set(group.retain());
                self.configure_native_quick_action_group();
            }
            return Some(group.into_super());
        }

        let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), identifier);

        match identifier_string.as_str() {
            TOOLBAR_WORKSPACE => {
                let active_workspace = self
                    .ivars()
                    .active_workspace_id
                    .borrow()
                    .as_deref()
                    .and_then(|workspace_id| {
                        self.ivars()
                            .workspaces
                            .borrow()
                            .iter()
                            .find(|candidate| candidate.selection_id() == workspace_id)
                            .cloned()
                    });
                let workspace_title = active_workspace
                    .as_ref()
                    .map(|workspace| workspace.label.clone())
                    .unwrap_or_else(|| "Workspace".to_string());
                let workspace_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("folder"),
                    Some(&NSString::from_str("Workspace")),
                )?;
                let workspace = unsafe {
                    NSButton::buttonWithTitle_image_target_action(
                        &NSString::from_str(&workspace_title),
                        &workspace_image,
                        Some(self),
                        Some(sel!(toggleWorkspacePicker:)),
                        mtm,
                    )
                };
                workspace.setBezelStyle(NSBezelStyle::Toolbar);
                workspace.setControlSize(NSControlSize::Regular);
                workspace.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                workspace.setImagePosition(NSCellImagePosition::ImageLeading);
                workspace.setImageHugsTitle(true);
                workspace.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
                workspace.setTranslatesAutoresizingMaskIntoConstraints(false);
                workspace.setContentCompressionResistancePriority_forOrientation(
                    NSLayoutPriorityDefaultLow,
                    NSLayoutConstraintOrientation::Horizontal,
                );
                workspace
                    .widthAnchor()
                    .constraintGreaterThanOrEqualToConstant(TOOLBAR_WORKSPACE_MIN_WIDTH)
                    .setActive(true);
                workspace
                    .widthAnchor()
                    .constraintLessThanOrEqualToConstant(TOOLBAR_WORKSPACE_MAX_WIDTH)
                    .setActive(true);
                workspace.setToolTip(Some(&NSString::from_str("Choose workspace")));
                workspace.setContentTintColor(
                    active_workspace
                        .as_ref()
                        .and_then(|workspace| workspace.workspace.color.as_ref())
                        .and_then(|color| ns_color_from_hex(&color.background))
                        .as_deref(),
                );
                let spinner = NSProgressIndicator::initWithFrame(
                    NSProgressIndicator::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(16.0, 16.0)),
                );
                spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                spinner.setControlSize(NSControlSize::Small);
                spinner.setIndeterminate(true);
                spinner.setDisplayedWhenStopped(false);
                spinner.setTranslatesAutoresizingMaskIntoConstraints(false);
                workspace.addSubview(&spinner);
                spinner
                    .centerYAnchor()
                    .constraintEqualToAnchor(&workspace.centerYAnchor())
                    .setActive(true);
                spinner
                    .leadingAnchor()
                    .constraintEqualToAnchor_constant(&workspace.leadingAnchor(), 10.0)
                    .setActive(true);
                spinner
                    .widthAnchor()
                    .constraintEqualToConstant(16.0)
                    .setActive(true);
                spinner
                    .heightAnchor()
                    .constraintEqualToConstant(16.0)
                    .setActive(true);
                item.setLabel(&NSString::from_str("Workspace"));
                item.setPaletteLabel(&NSString::from_str("Workspace"));
                item.setVisibilityPriority(NSToolbarItemVisibilityPriorityHigh);
                item.setView(Some(&workspace));
                if will_be_inserted && self.ivars().workspace_button.get().is_none() {
                    let _ = self.ivars().workspace_button.set(workspace);
                    let _ = self.ivars().workspace_button_spinner.set(spinner);
                    self.refresh_workspace_loading_indicators();
                }
            }
            TOOLBAR_BRANCH => {
                let branch_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("arrow.triangle.branch"),
                    Some(&NSString::from_str("Branch")),
                )?;
                // SAFETY: The target implements toggleBranchPicker: with an NSButton sender.
                let branch = unsafe {
                    NSButton::buttonWithTitle_image_target_action(
                        &NSString::from_str("Branch"),
                        &branch_image,
                        Some(self),
                        Some(sel!(toggleBranchPicker:)),
                        mtm,
                    )
                };
                branch.setBezelStyle(NSBezelStyle::Toolbar);
                branch.setControlSize(NSControlSize::Regular);
                branch.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                branch.setImagePosition(NSCellImagePosition::ImageLeading);
                branch.setImageHugsTitle(true);
                branch.setToolTip(Some(&NSString::from_str("Branch")));
                branch.setEnabled(false);
                item.setLabel(&NSString::from_str("Branch"));
                item.setPaletteLabel(&NSString::from_str("Branch"));
                item.setVisibilityPriority(NSToolbarItemVisibilityPriorityLow);
                item.setView(Some(&branch));
                if will_be_inserted && self.ivars().branch_button.get().is_none() {
                    let _ = self.ivars().branch_button.set(branch);
                }
            }
            TOOLBAR_FETCH => {
                let label = NSString::from_str("Fetch remote");
                // The refresh SF Symbol reaches its trailing alignment edge; an en-space keeps
                // the visible title from touching the arrowhead while preserving native sizing.
                let visible_label = NSString::from_str("\u{2002}Fetch remote");
                let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("arrow.triangle.2.circlepath"),
                    Some(&label),
                )?;
                // SAFETY: The target implements fetchRemote: with an NSButton sender.
                let button = unsafe {
                    NSButton::buttonWithTitle_image_target_action(
                        &visible_label,
                        &image,
                        Some(self),
                        Some(sel!(fetchRemote:)),
                        mtm,
                    )
                };
                button.setBezelStyle(NSBezelStyle::Toolbar);
                button.setControlSize(NSControlSize::Regular);
                button.setImagePosition(NSCellImagePosition::ImageLeading);
                button.setImageHugsTitle(true);
                button.setToolTip(Some(&NSString::from_str("Last fetched: unknown")));
                button.setEnabled(false);
                let spinner = NSProgressIndicator::initWithFrame(
                    NSProgressIndicator::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(16.0, 16.0)),
                );
                spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                spinner.setControlSize(NSControlSize::Small);
                spinner.setIndeterminate(true);
                spinner.setDisplayedWhenStopped(false);
                spinner.setTranslatesAutoresizingMaskIntoConstraints(false);
                button.addSubview(&spinner);
                spinner
                    .centerYAnchor()
                    .constraintEqualToAnchor(&button.centerYAnchor())
                    .setActive(true);
                spinner
                    .leadingAnchor()
                    .constraintEqualToAnchor_constant(&button.leadingAnchor(), 10.0)
                    .setActive(true);
                spinner
                    .widthAnchor()
                    .constraintEqualToConstant(16.0)
                    .setActive(true);
                spinner
                    .heightAnchor()
                    .constraintEqualToConstant(16.0)
                    .setActive(true);
                item.setLabel(&label);
                item.setPaletteLabel(&label);
                item.setVisibilityPriority(NSToolbarItemVisibilityPriorityLow);
                item.setView(Some(&button));
                if will_be_inserted && self.ivars().fetch_button.get().is_none() {
                    let _ = self.ivars().fetch_button.set(button);
                    let _ = self.ivars().fetch_spinner.set(spinner);
                }
            }
            TOOLBAR_TERMINAL => {
                let (label, symbol, tooltip, action, enabled) = (
                    "Terminal",
                    "apple.terminal",
                    "Show terminal",
                    sel!(toggleTerminal:),
                    true,
                );
                let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str(label)),
                )
                .or_else(|| {
                    NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str("terminal"),
                        Some(&NSString::from_str(label)),
                    )
                })?;
                item.setLabel(&NSString::from_str(label));
                item.setPaletteLabel(&NSString::from_str(label));
                item.setToolTip(Some(&NSString::from_str(tooltip)));
                item.setImage(Some(&image));
                item.setBordered(true);
                item.setEnabled(enabled);
                unsafe {
                    item.setTarget(Some(self));
                    item.setAction(Some(action));
                }
                if will_be_inserted && self.ivars().terminal_toolbar_item.get().is_none() {
                    let _ = self.ivars().terminal_toolbar_item.set(item.clone());
                }
            }
            _ => return None,
        }
        Some(item)
    }

}
