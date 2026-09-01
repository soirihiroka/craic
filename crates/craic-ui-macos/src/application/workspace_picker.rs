impl AppDelegate {
    fn show_workspace_picker(&self, sender: &NSButton) {
        let popover = self.workspace_picker();
        if popover.isShown() {
            popover.close();
            return;
        }
        let preferred = self
            .ivars()
            .active_workspace_id
            .borrow()
            .as_deref()
            .and_then(|workspace_id| {
                self.ivars()
                    .workspaces
                    .borrow()
                    .iter()
                    .find(|entry| entry.selection_id() == workspace_id)
                    .map(|entry| entry.workspace.clone())
            });
        self.request_workspace_discovery(preferred, false);
        if let Some(search) = self.ivars().workspace_search.get() {
            search.setStringValue(&NSString::new());
        }
        self.refresh_workspace_results("");
        popover.showRelativeToRect_ofView_preferredEdge(sender.bounds(), sender, NSRectEdge::MinY);
        if let Some(search) = self.ivars().workspace_search.get()
            && let Some(window) = search.window()
        {
            window.makeFirstResponder(Some(search));
        }
    }

    fn request_workspace_discovery(
        &self,
        preferred: Option<craic_config::ConfiguredWorkspace>,
        select_workspace: bool,
    ) {
        if self.ivars().workspace_discovery_loading.get() {
            return;
        }
        let Some(requests) = self.ivars().workspace_discovery_requests.get() else {
            log::warn!("workspace discovery ignored because the discovery service is unavailable");
            return;
        };
        let generation = self
            .ivars()
            .workspace_discovery_generation
            .get()
            .wrapping_add(1);
        self.ivars().workspace_discovery_generation.set(generation);
        self.ivars().workspace_discovery_loading.set(true);
        self.refresh_workspace_loading_indicators();
        if let Err(error) = requests.try_send(WorkspaceDiscoveryRequest {
            generation,
            preferred,
            select_workspace,
        }) {
            self.ivars().workspace_discovery_loading.set(false);
            self.refresh_workspace_loading_indicators();
            log::warn!("workspace discovery queue rejected generation={generation} error={error}");
        }
    }

    fn activate_workspace_at(&self, index: usize) {
        let Some(workspace) = self.ivars().workspaces.borrow().get(index).cloned() else {
            log::warn!("workspace selection index out of range index={index}");
            return;
        };
        self.prompt_workspace_open(workspace, None);
    }

    fn prompt_workspace_open(&self, workspace: WorkspaceEntry, message: Option<String>) {
        if let Some(popover) = self.ivars().workspace_popover.get() {
            popover.close();
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Open Workspace"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "Open {} in this window or a new window?",
            workspace.label
        )));
        alert.addButtonWithTitle(&NSString::from_str("Open Here"));
        alert.addButtonWithTitle(&NSString::from_str("Open in New Window"));
        let cancel = alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response == NSAlertFirstButtonReturn {
                delegate.activate_workspace_here(workspace.clone());
                if let Some(message) = message
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    delegate.show_native_toast(message);
                }
            } else if response == NSAlertSecondButtonReturn {
                delegate.open_workspace_in_new_window(&workspace.workspace);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn activate_workspace_here(&self, workspace: WorkspaceEntry) {
        self.ivars().workspace_discovery_generation.set(
            self.ivars()
                .workspace_discovery_generation
                .get()
                .wrapping_add(1),
        );
        self.ivars().workspace_discovery_loading.set(false);
        self.refresh_workspace_loading_indicators();
        let selection = WorkspaceSelection {
            id: WorkspaceId::new(workspace.selection_id()),
        };
        let Some(handle) = self.ivars().app_handle.get() else {
            log::warn!("workspace selection ignored because application actor is unavailable");
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::SelectWorkspace(selection)) {
            log::warn!("workspace selection queue rejected command={command:?}");
            return;
        }
        self.apply_workspace_button_appearance(&workspace);
        self.begin_workspace_transition(&workspace.selection_id());
        self.request_repository_load(workspace.workspace.clone());
        self.queue_save_last_workspace(workspace.workspace);
    }

    fn open_workspace_in_new_window(&self, workspace: &craic_config::ConfiguredWorkspace) {
        let provider_flag = NSString::from_str("--workspace-provider");
        let provider = NSString::from_str(&workspace.provider_id());
        let path_flag = NSString::from_str("--workspace-path");
        let path = NSString::from_str(&workspace.path);
        let argument_refs: [&NSString; 4] = [&provider_flag, &provider, &path_flag, &path];
        let arguments = NSArray::from_slice(&argument_refs);
        let configuration = NSWorkspaceOpenConfiguration::configuration();
        configuration.setCreatesNewApplicationInstance(true);
        configuration.setArguments(&arguments);
        let label = workspace.label();
        let delegate = Arc::new(MainThreadBound::new(self.retain(), self.mtm()));
        let completion = RcBlock::new(
            move |_application: *mut NSRunningApplication, error: *mut NSError| {
                let Some(error) = (unsafe { error.as_ref() }) else {
                    log::info!("workspace opened in new window label={label}");
                    return;
                };
                let message = error.localizedDescription().to_string();
                log::warn!("workspace new-window launch failed label={label} error={message}");
                let delegate = delegate.clone();
                DispatchQueue::main().exec_async(move || {
                    let Some(mtm) = MainThreadMarker::new() else {
                        return;
                    };
                    delegate
                        .get(mtm)
                        .present_path_action_error("Open Workspace Failed", &message);
                });
            },
        );
        NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
            &NSBundle::mainBundle().bundleURL(),
            &configuration,
            Some(&completion),
        );
    }

    fn queue_save_last_workspace(&self, workspace: craic_config::ConfiguredWorkspace) {
        let Some(requests) = self.ivars().frontend_requests.get() else {
            log::warn!("last workspace was not saved because the frontend service is unavailable");
            return;
        };
        if let Err(error) = requests.try_send(FrontendRequest::SaveLastWorkspace(workspace)) {
            log::warn!("last-workspace save queue rejected request error={error}");
        }
    }

    fn workspace_picker(&self) -> Retained<NSPopover> {
        if let Some(popover) = self.ivars().workspace_popover.get() {
            return popover.clone();
        }

        let mtm = self.mtm();
        let root = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(WORKSPACE_PICKER_WIDTH, WORKSPACE_PICKER_HEIGHT),
            ),
        );
        let search = NSSearchField::new(mtm);
        search.setFrame(NSRect::new(
            NSPoint::new(12.0, WORKSPACE_PICKER_HEIGHT - 44.0),
            NSSize::new(WORKSPACE_PICKER_WIDTH - 62.0, 32.0),
        ));
        search.setPlaceholderString(Some(&NSString::from_str("Search workspaces")));
        search.setSendsSearchStringImmediately(true);
        search.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        unsafe {
            search.setTarget(Some(self));
            search.setAction(Some(sel!(filterWorkspaces:)));
        }
        root.addSubview(&search);

        let add = unsafe {
            NSButton::buttonWithImage_target_action(
                &NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("plus"),
                    Some(&NSString::from_str("Create workspace")),
                )
                .expect("macOS 14 provides the plus SF Symbol"),
                Some(self),
                Some(sel!(addWorkspace:)),
                mtm,
            )
        };
        add.setFrame(NSRect::new(
            NSPoint::new(
                WORKSPACE_PICKER_WIDTH - 44.0,
                WORKSPACE_PICKER_HEIGHT - 44.0,
            ),
            NSSize::new(32.0, 32.0),
        ));
        add.setBezelStyle(NSBezelStyle::Circular);
        add.setControlSize(NSControlSize::Regular);
        add.setToolTip(Some(&NSString::from_str("Create workspace")));
        add.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        root.addSubview(&add);
        self.ivars()
            .workspace_add_button
            .set(add)
            .expect("workspace add button is initialized once");

        let table = NSTableView::initWithFrame(
            NSTableView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(
                    WORKSPACE_PICKER_WIDTH - 24.0,
                    WORKSPACE_PICKER_HEIGHT - 68.0,
                ),
            ),
        );
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(mtm),
            &NSUserInterfaceItemIdentifier::from_str("workspace.result"),
        );
        column.setWidth(WORKSPACE_PICKER_WIDTH - 24.0);
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        table.setStyle(NSTableViewStyle::Inset);
        table.setRowHeight(WORKSPACE_ROW_HEIGHT);
        table.setIntercellSpacing(NSSize::new(0.0, 2.0));
        table.setUsesAlternatingRowBackgroundColors(false);
        table.setAllowsEmptySelection(true);
        table.setAllowsMultipleSelection(false);
        table.setAllowsTypeSelect(true);
        table.setBackgroundColor(&NSColor::clearColor());
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
            table.setTarget(Some(self));
            table.setAction(Some(sel!(activateWorkspaceRow:)));
        }
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(12.0, 12.0),
                NSSize::new(
                    WORKSPACE_PICKER_WIDTH - 24.0,
                    WORKSPACE_PICKER_HEIGHT - 68.0,
                ),
            ),
        );
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setAutomaticallyAdjustsContentInsets(true);
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(false);
        scroll.setAutohidesScrollers(true);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setDocumentView(Some(&table));
        root.addSubview(&scroll);

        let controller = NSViewController::new(mtm);
        controller.setView(&root);
        controller
            .setPreferredContentSize(NSSize::new(WORKSPACE_PICKER_WIDTH, WORKSPACE_PICKER_HEIGHT));
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(NSSize::new(WORKSPACE_PICKER_WIDTH, WORKSPACE_PICKER_HEIGHT));
        popover.setContentViewController(Some(&controller));

        self.ivars()
            .workspace_search
            .set(search)
            .expect("workspace search is initialized once");
        self.ivars()
            .workspace_table
            .set(table)
            .expect("workspace result table is initialized once");
        self.ivars()
            .workspace_popover
            .set(popover.clone())
            .expect("workspace popover is initialized once");
        popover
    }

    fn refresh_workspace_results(&self, filter: &str) {
        let Some(table) = self.ivars().workspace_table.get() else {
            return;
        };

        let terms = filter
            .trim()
            .to_lowercase()
            .split(|character: char| !character.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let workspaces = self.ivars().workspaces.borrow();
        let matches = workspaces
            .iter()
            .enumerate()
            .filter(|(_, workspace)| {
                if terms.is_empty() {
                    return true;
                }
                let candidate = format!(
                    "{} {} {} {}",
                    workspace.label,
                    workspace.workspace.path,
                    workspace.selection_id(),
                    self.ivars()
                        .workspace_metadata
                        .borrow()
                        .get(&workspace.selection_id())
                        .and_then(|metadata| metadata.remote_label.as_deref())
                        .unwrap_or_default()
                )
                .to_lowercase();
                terms.iter().all(|term| candidate.contains(term))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        drop(workspaces);
        self.ivars().workspace_results.replace(matches);
        table.reloadData();
        let active = self.ivars().active_workspace_id.borrow();
        let selected = self
            .ivars()
            .workspace_results
            .borrow()
            .iter()
            .position(|index| {
                self.ivars()
                    .workspaces
                    .borrow()
                    .get(*index)
                    .is_some_and(|workspace| {
                        active.as_deref() == Some(workspace.selection_id().as_str())
                    })
            });
        if let Some(selected) = selected {
            table.selectRowIndexes_byExtendingSelection(
                &NSIndexSet::indexSetWithIndex(selected),
                false,
            );
            table.scrollRowToVisible(selected as isize);
        } else {
            // SAFETY: This table is owned and updated exclusively on AppKit's main thread.
            unsafe { table.deselectAll(None) };
        }
    }

    fn make_workspace_cell(&self, table: &NSTableView, row: usize) -> Option<Retained<NSView>> {
        let width = table.bounds().size.width.max(WORKSPACE_PICKER_WIDTH - 24.0);
        let workspace_index = self.ivars().workspace_results.borrow().get(row).copied();
        let workspace =
            workspace_index.and_then(|index| self.ivars().workspaces.borrow().get(index).cloned());
        let (Some(workspace_index), Some(workspace)) = (workspace_index, workspace) else {
            if !self.ivars().workspace_results.borrow().is_empty() {
                return None;
            }
            let cell = NSView::initWithFrame(
                NSView::alloc(self.mtm()),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(width, WORKSPACE_ROW_HEIGHT),
                ),
            );
            let loading = self.ivars().workspace_discovery_loading.get();
            let empty = NSTextField::labelWithString(
                &NSString::from_str(if loading {
                    "Loading workspaces…"
                } else {
                    "No workspaces found"
                }),
                self.mtm(),
            );
            empty.setFrame(NSRect::new(
                NSPoint::new(12.0, 14.0),
                NSSize::new(width - 24.0, 18.0),
            ));
            empty.setAlignment(NSTextAlignment::Center);
            empty.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            empty.setTextColor(Some(&NSColor::secondaryLabelColor()));
            cell.addSubview(&empty);
            if loading {
                let spinner = NSProgressIndicator::initWithFrame(
                    NSProgressIndicator::alloc(self.mtm()),
                    NSRect::new(
                        NSPoint::new((width - 170.0).max(8.0) / 2.0, 15.0),
                        NSSize::new(16.0, 16.0),
                    ),
                );
                spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                spinner.setControlSize(NSControlSize::Small);
                spinner.setIndeterminate(true);
                spinner.setDisplayedWhenStopped(false);
                unsafe { spinner.startAnimation(None) };
                cell.addSubview(&spinner);
            }
            return Some(cell);
        };

        let cell = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, WORKSPACE_ROW_HEIGHT),
            ),
        );
        let workspace_color = workspace
            .workspace
            .color
            .as_ref()
            .and_then(|color| ns_color_from_hex(&color.background));
        let metadata = self
            .ivars()
            .workspace_metadata
            .borrow()
            .get(&workspace.selection_id())
            .cloned();
        let metadata_loading = self
            .ivars()
            .workspace_metadata_pending
            .borrow()
            .contains(&workspace.selection_id());
        if metadata_loading {
            let spinner = NSProgressIndicator::initWithFrame(
                NSProgressIndicator::alloc(self.mtm()),
                NSRect::new(NSPoint::new(11.0, 15.0), NSSize::new(16.0, 16.0)),
            );
            spinner.setStyle(NSProgressIndicatorStyle::Spinning);
            spinner.setControlSize(NSControlSize::Small);
            spinner.setIndeterminate(true);
            spinner.setDisplayedWhenStopped(false);
            unsafe { spinner.startAnimation(None) };
            cell.addSubview(&spinner);
        } else {
            let symbol = metadata
                .as_ref()
                .map(|metadata| native_workspace_metadata_symbol(metadata.kind))
                .unwrap_or("folder");
            if let Some(folder) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(
                    metadata
                        .as_ref()
                        .map(|metadata| native_workspace_metadata_description(metadata.kind))
                        .unwrap_or("Workspace"),
                )),
            ) {
                let icon = NSImageView::imageViewWithImage(&folder, self.mtm());
                let icon_size = 18.0;
                icon.setFrame(NSRect::new(
                    NSPoint::new(10.0, 14.0),
                    NSSize::new(icon_size, icon_size),
                ));
                icon.setContentTintColor(Some(
                    workspace_color
                        .as_deref()
                        .unwrap_or(&NSColor::secondaryLabelColor()),
                ));
                cell.addSubview(&icon);
            }
        }

        if let Some(workspace_color) = workspace_color.as_deref() {
            let dot = NSView::initWithFrame(
                NSView::alloc(self.mtm()),
                NSRect::new(NSPoint::new(24.0, 12.0), NSSize::new(6.0, 6.0)),
            );
            dot.setWantsLayer(true);
            if let Some(layer) = dot.layer() {
                layer.setBackgroundColor(Some(&workspace_color.CGColor()));
                layer.setCornerRadius(3.0);
            }
            cell.addSubview(&dot);
        }

        let is_current = self
            .ivars()
            .active_workspace_id
            .borrow()
            .as_deref()
            .is_some_and(|active| workspace.selection_id() == active);
        let trailing_width = if is_current { 72.0 } else { 12.0 };
        let display_label = metadata
            .as_ref()
            .and_then(|metadata| metadata.remote_label.as_deref())
            .unwrap_or(&workspace.label);
        let title = NSTextField::labelWithString(&NSString::from_str(display_label), self.mtm());
        title.setFrame(NSRect::new(
            NSPoint::new(36.0, 23.0),
            NSSize::new((width - 36.0 - trailing_width).max(1.0), 17.0),
        ));
        title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        title.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        cell.addSubview(&title);

        let path = workspace.workspace.path.as_str();
        let subtitle = NSTextField::labelWithString(&NSString::from_str(&path), self.mtm());
        subtitle.setFrame(NSRect::new(
            NSPoint::new(36.0, 6.0),
            NSSize::new((width - 36.0 - trailing_width).max(1.0), 15.0),
        ));
        subtitle.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        subtitle.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
        subtitle.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        subtitle.setToolTip(Some(&NSString::from_str(&path)));
        cell.addSubview(&subtitle);

        if is_current {
            let current =
                NSTextField::labelWithString(&NSString::from_str("✓ Current"), self.mtm());
            current.setFrame(NSRect::new(
                NSPoint::new(width - 70.0, 15.0),
                NSSize::new(62.0, 17.0),
            ));
            current.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            current.setAlignment(NSTextAlignment::Right);
            current.setFont(Some(&NSFont::boldSystemFontOfSize(10.5)));
            current.setTextColor(Some(&NSColor::controlAccentColor()));
            current.setToolTip(Some(&NSString::from_str("Current workspace")));
            cell.addSubview(&current);
        }
        cell.setToolTip(Some(&NSString::from_str(&format!(
            "Open {} at {}",
            workspace.label, path
        ))));
        let activation = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&format!("Open {}", workspace.label)),
                Some(self),
                Some(sel!(activateWorkspaceOption:)),
                self.mtm(),
            )
        };
        activation.setTag(workspace_index as isize);
        activation.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, WORKSPACE_ROW_HEIGHT),
        ));
        activation.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        activation.setBordered(false);
        activation.setTransparent(true);
        activation.setToolTip(Some(&NSString::from_str(&format!(
            "Open {} at {}",
            workspace.label, path
        ))));
        cell.addSubview(&activation);
        Some(cell)
    }

    fn queue_workspace_metadata(&self, entries: Vec<WorkspaceEntry>) {
        if entries.is_empty() {
            return;
        }
        let workspace_ids = entries
            .iter()
            .map(WorkspaceEntry::selection_id)
            .collect::<Vec<_>>();
        self.ivars()
            .workspace_metadata_pending
            .borrow_mut()
            .extend(workspace_ids.iter().cloned());
        self.refresh_workspace_loading_indicators();
        let generation = self.ivars().workspace_metadata_generation.get();
        let Some(requests) = self.ivars().workspace_metadata_requests.get() else {
            log::warn!("workspace metadata was not queued because its service is unavailable");
            let mut pending = self.ivars().workspace_metadata_pending.borrow_mut();
            for workspace_id in &workspace_ids {
                pending.remove(workspace_id);
            }
            drop(pending);
            self.refresh_workspace_loading_indicators();
            return;
        };
        if let Err(error) = requests.try_send(NativeWorkspaceMetadataRequest {
            generation,
            entries,
        }) {
            log::warn!(
                "workspace metadata batch queue rejected generation={generation} error={error}"
            );
            let mut pending = self.ivars().workspace_metadata_pending.borrow_mut();
            for workspace_id in &workspace_ids {
                pending.remove(workspace_id);
            }
            drop(pending);
            self.refresh_workspace_loading_indicators();
        }
    }

    fn apply_workspace_metadata(
        &self,
        workspace_id: &str,
        generation: u64,
        result: Result<NativeWorkspaceMetadata, String>,
    ) {
        if self.ivars().workspace_metadata_generation.get() != generation
            || !self
                .ivars()
                .workspaces
                .borrow()
                .iter()
                .any(|workspace| workspace.selection_id() == workspace_id)
        {
            log::debug!(
                "discarding stale workspace metadata workspace={workspace_id} generation={generation}"
            );
            return;
        }
        self.ivars()
            .workspace_metadata_pending
            .borrow_mut()
            .remove(workspace_id);
        match result {
            Ok(metadata) => {
                log::debug!(
                    "native workspace metadata applied workspace={workspace_id} generation={generation} kind={:?} label={}",
                    metadata.kind,
                    metadata.remote_label.as_deref().unwrap_or_default()
                );
                self.ivars()
                    .workspace_metadata
                    .borrow_mut()
                    .insert(workspace_id.to_string(), metadata);
            }
            Err(error) => {
                log::warn!("workspace metadata failed workspace={workspace_id}: {error}");
            }
        }
        let filter = self
            .ivars()
            .workspace_search
            .get()
            .map(|search| search.stringValue().to_string())
            .unwrap_or_default();
        self.refresh_workspace_results(&filter);
        if self.ivars().active_workspace_id.borrow().as_deref() == Some(workspace_id)
            && let Some(workspace) = self
                .ivars()
                .workspaces
                .borrow()
                .iter()
                .find(|workspace| workspace.selection_id() == workspace_id)
        {
            self.apply_workspace_button_appearance(workspace);
        }
        self.refresh_workspace_loading_indicators();
    }

}
