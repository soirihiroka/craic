impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        // SAFETY: This invokes NSObject's valid initializer for a newly allocated subclass.
        let delegate: Retained<Self> = unsafe { msg_send![super(this), init] };
        delegate
            .ivars()
            .font_sizes
            .set(craic_config::load().font_sizes);
        delegate
    }

    fn workspace_cancellation_token(&self) -> Option<WorkspaceCancellationToken> {
        self.ivars()
            .app_handle
            .get()
            .map(AppHandle::workspace_cancellation_token)
    }

    fn request_remote_action(&self, action: NativeRemoteAction) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            return;
        };
        let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            log::warn!("git action ignored because workspace cancellation is unavailable");
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            log::warn!("git action ignored because repository service is unavailable");
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::RunGitAction {
            workspace_id: workspace_id.clone(),
            handle,
            snapshot,
            action,
            stash_before: false,
            cancellation,
        }) {
            self.repository_action_failed(
                &workspace_id,
                "Git Operation Failed",
                &format!("The Git operation could not be queued: {error}"),
            );
        }
    }

    fn show_native_shortcuts_window(&self) {
        if let Some(window) = self.ivars().shortcuts_window.borrow().as_ref() {
            window.makeKeyAndOrderFront(Some(self));
            return;
        }

        let size = NSSize::new(520.0, 540.0);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(self.mtm()),
                NSRect::new(NSPoint::ZERO, size),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Craic Keyboard Shortcuts"));
        window.setContentMinSize(NSSize::new(420.0, 360.0));

        let text = NSTextView::initWithFrame(
            NSTextView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, size),
        );
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(false);
        text.setDrawsBackground(false);
        text.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        text.setTextContainerInset(NSSize::new(24.0, 20.0));
        text.setString(&NSString::from_str(
            "APPLICATION\n⌘N   New Window\n⌘O   Open Workspace\n⌘,   Settings\n⌘?   Keyboard Shortcuts\n\nNAVIGATION\n⌘1   Changes\n⌘2   History\n⌘3   Files\n⌘4   Containers\n⌘5   Agents\n⌘F   Find in the active view\n⌘R   Refresh Workspace\n⇧⌘R  Refresh Current Page\n\nSOURCE CONTROL\n⌘P   Pull Remote Changes\n⌘U   Push Local Commits\n\nFILES\n↑/↓  Select entry\nReturn  Open or expand entry\n⌘C   Copy workspace entry\n⌘X   Cut workspace entry\n⌘V   Paste workspace entry\nDelete  Delete with confirmation\n\nEDITING\n⌘Z   Undo\n⇧⌘Z  Redo\n⌘/   Toggle Line Comment\n⌘X   Cut\n⌘C   Copy\n⌘V   Paste\n⌘A   Select All\n\nTEXT SIZE\n⇧⌘=  Increase focused surface\n⌘-   Decrease focused surface\n⌘0   Reset focused surface\n\nTERMINAL AND CHAT\nReturn sends a chat message. Shift-Return inserts a newline.\nThe integrated terminal follows standard macOS copy, paste, selection, and find behavior.\n",
        ));

        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, size),
        );
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setDocumentView(Some(&text));
        window.setContentView(Some(&scroll));
        window.center();
        window.makeKeyAndOrderFront(Some(self));
        self.ivars().shortcuts_window.replace(Some(window));
    }

    fn open_native_help_url(&self, value: &str) {
        let Some(url) = NSURL::URLWithString(&NSString::from_str(value)) else {
            log::warn!("native help URL is invalid url={value}");
            return;
        };
        if !NSWorkspace::sharedWorkspace().openURL(&url) {
            self.present_path_action_error("Unable to Open Link", value);
        }
    }

    fn request_changes_refresh(&self, page_request_id: String) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            self.complete_pending_page_service(
                "changes",
                Err("No workspace is active".to_string()),
            );
            return;
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            self.complete_pending_page_service(
                "changes",
                Err("The workspace is not loaded".to_string()),
            );
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            self.complete_pending_page_service(
                "changes",
                Err("The workspace is shutting down".to_string()),
            );
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.changes_operation_failed(
                "Refresh Failed",
                "The repository service is unavailable.",
            );
            self.complete_pending_page_service(
                "changes",
                Err("The repository service is unavailable".to_string()),
            );
            return;
        };
        self.set_page_badge("changes", NativePageBadge::Indicator);
        self.set_repository_action_progress(&workspace_id, "Refreshing…");
        if let Err(error) = requests.try_send(RepositoryRequest::Refresh {
            workspace_id: workspace_id.clone(),
            handle,
            core_request: Some(RepositoryCoreRefreshRequest::Page(page_request_id)),
            cancellation,
        }) {
            self.repository_action_failed(
                &workspace_id,
                "Git Operation Failed",
                &error.to_string(),
            );
            self.changes_operation_failed(
                "Refresh Failed",
                &format!("Refresh request could not be queued: {error}"),
            );
            self.restore_changes_page_badge();
            self.complete_pending_page_service("changes", Err(error.to_string()));
        }
    }

    fn request_workspace_refresh(&self, request: WorkspaceRefreshRequest) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            self.complete_workspace_refresh(request.identity, Err("No workspace is active".into()));
            return;
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            self.complete_workspace_refresh(
                request.identity,
                Err("The workspace is not loaded".into()),
            );
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            self.complete_workspace_refresh(
                request.identity,
                Err("The workspace is shutting down".into()),
            );
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.complete_workspace_refresh(
                request.identity,
                Err("The repository service is unavailable".into()),
            );
            return;
        };
        self.set_repository_action_progress(&workspace_id, "Refreshing…");
        if let Err(error) = requests.try_send(RepositoryRequest::Refresh {
            workspace_id: workspace_id.clone(),
            handle,
            core_request: Some(RepositoryCoreRefreshRequest::Workspace(request.clone())),
            cancellation,
        }) {
            self.repository_action_failed(&workspace_id, "Refresh Failed", &error.to_string());
            self.complete_workspace_refresh(request.identity, Err(error.to_string()));
        }
    }

    fn select_commit_author_at(&self, index: usize) {
        let Some(option) = self.ivars().author_options.borrow().get(index).cloned() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::SaveCommitAuthor {
            workspace_id,
            handle,
            option,
            cancellation,
        }) {
            self.present_path_action_error(
                "Author Selection Failed",
                &format!("Unable to queue author update: {error}"),
            );
        }
    }

    fn update_diff_search_status(&self) {
        let Some(status) = self.ivars().diff_search_status.get() else {
            return;
        };
        let text = self
            .ivars()
            .diff_view
            .get()
            .map(|view| view.search_status())
            .unwrap_or_default();
        status.setStringValue(&NSString::from_str(&text));
    }

    fn layout_sidebar(&self) {
        let (
            Some(sidebar),
            Some(changes_split),
            Some(changes_browser),
            Some(edge_container),
            Some(edge_height_constraint),
            Some(top_cover),
            Some(search_popup),
            Some(search_panel),
            Some(changes_scroll),
        ) = (
            self.ivars().sidebar.get(),
            self.ivars().changes_split.get(),
            self.ivars().changes_browser.get(),
            self.ivars().changes_edge_container.get(),
            self.ivars().changes_edge_height.get(),
            self.ivars().changes_top_cover.get(),
            self.ivars().changes_search_popup.get(),
            self.ivars().changes_search_panel.get(),
            self.ivars().changes_scroll.get(),
        )
        else {
            return;
        };
        let sidebar_bounds = sidebar.bounds();
        changes_split.setFrame(sidebar_bounds);
        changes_split.adjustSubviews();
        let browser_bounds = changes_browser.bounds();
        let safe = changes_browser.safeAreaInsets();
        let count_width = 220.0_f64.min((sidebar_bounds.size.width - 96.0).max(1.0));
        top_cover.setFrame(NSRect::new(
            NSPoint::new(
                sidebar_bounds.size.width - count_width - 12.0,
                sidebar_bounds.size.height - SELECTION_HEADER_HEIGHT - 8.0,
            ),
            NSSize::new(count_width, SELECTION_HEADER_HEIGHT),
        ));
        if let Some(count_content) = top_cover.contentView() {
            count_content.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(count_width, SELECTION_HEADER_HEIGHT),
            ));
        }
        let search_visible = self.ivars().changes_search_visible.get();
        let cover_margin = 8.0;
        let search_popup_height = 46.0;
        let selection_header_inset = SELECTION_HEADER_HEIGHT + 16.0;
        let search_accessory_visible = self.is_active_page("changes") && search_visible;
        let search_accessory_height = if search_accessory_visible { 62.0 } else { 0.0 };
        edge_container.setHidden(!search_accessory_visible);
        if let Some(edge_accessory) = self.ivars().changes_edge_accessory.get() {
            unsafe {
                let _: () = msg_send![&**edge_accessory, setHidden: !search_accessory_visible];
            }
        }
        edge_height_constraint.setConstant(search_accessory_height);
        edge_container.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(sidebar_bounds.size.width, search_accessory_height),
        ));
        let search_width = 360.0_f64.min((sidebar_bounds.size.width - 24.0).max(1.0));
        search_popup.setHidden(!search_visible);
        search_popup.setFrame(NSRect::new(
            NSPoint::new(
                sidebar_bounds.size.width - search_width - 12.0,
                cover_margin,
            ),
            NSSize::new(search_width, search_popup_height),
        ));
        if let Some(search_content) = search_popup.contentView() {
            search_content.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(search_width, search_popup_height),
            ));
        }
        search_panel.setHidden(!search_visible);
        search_panel.setFrame(NSRect::new(
            NSPoint::new(0.0, 4.0),
            NSSize::new(search_width, 38.0),
        ));
        let scroll_top = if search_visible {
            (browser_bounds.size.height - search_accessory_height).max(safe.bottom)
        } else {
            browser_bounds.size.height
        };
        changes_scroll.setFrame(NSRect::new(
            NSPoint::new(10.0, safe.bottom),
            NSSize::new(
                (browser_bounds.size.width - 20.0).max(1.0),
                (scroll_top - safe.bottom).max(1.0),
            ),
        ));
        changes_scroll.setContentInsets(NSEdgeInsets {
            top: if search_visible {
                0.0
            } else {
                selection_header_inset
            },
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        let changes_clip = changes_scroll.contentView();
        let current_changes_bounds = changes_clip.bounds();
        let changes_content_insets = changes_scroll.contentInsets();
        let changes_document_height = changes_scroll
            .documentView()
            .map(|document| document.frame().size.height);
        let proposed_changes_origin = changes_document_height
            .filter(|height| {
                *height <= current_changes_bounds.size.height + changes_content_insets.top
            })
            .map_or(current_changes_bounds.origin, |height| {
                NSPoint::new(current_changes_bounds.origin.x, height)
            });
        let constrained_changes_bounds = changes_clip.constrainBoundsRect(NSRect::new(
            proposed_changes_origin,
            current_changes_bounds.size,
        ));
        log::debug!(
            "native changes scroll layout search={} document_height={:?} clip_height={} inset_top={} current_y={} proposed_y={} constrained_y={}",
            search_visible,
            changes_document_height,
            current_changes_bounds.size.height,
            changes_content_insets.top,
            current_changes_bounds.origin.y,
            proposed_changes_origin.y,
            constrained_changes_bounds.origin.y
        );
        changes_clip.scrollToPoint(constrained_changes_bounds.origin);
        changes_scroll.reflectScrolledClipView(&changes_clip);
        if let Some(history) = self.ivars().history.get() {
            history.sidebar_root.setFrame(sidebar_bounds);
            let history_safe = sidebar.safeAreaInsets();
            let history_top = sidebar_bounds.size.height - history_safe.top;
            let history_search_height = if self.ivars().history_search_visible.get() {
                38.0
            } else {
                0.0
            };
            history.search.setHidden(history_search_height == 0.0);
            history.search.setFrame(NSRect::new(
                NSPoint::new(12.0, (history_top - 38.0).max(history_safe.bottom + 52.0)),
                NSSize::new((sidebar_bounds.size.width - 24.0).max(1.0), 30.0),
            ));
            let history_scroll_top = if history_search_height == 0.0 {
                // Fill the split item instead of stopping at its safe-area edge. AppKit's
                // split accessory then applies its native scroll-edge fade to content moving
                // beneath the floating page switcher, while NSScrollView keeps resting content
                // clear of the accessory through its automatic content inset.
                sidebar_bounds.size.height
            } else {
                history_top - history_search_height
            };
            history.scroll.setFrame(NSRect::new(
                NSPoint::new(8.0, history_safe.bottom),
                NSSize::new(
                    (sidebar_bounds.size.width - 16.0).max(1.0),
                    (history_scroll_top - history_safe.bottom).max(1.0),
                ),
            ));
            let history_content_width = history.scroll.contentSize().width.max(1.0);
            let history_table_frame = history.table.frame();
            history.table.setFrameSize(NSSize::new(
                history_content_width,
                history_table_frame.size.height,
            ));
            history.table.sizeLastColumnToFit();
            let history_clip = history.scroll.contentView();
            let history_clip_origin = history_clip.bounds().origin;
            if history_clip_origin.x != 0.0 {
                history_clip.setBoundsOrigin(NSPoint::new(0.0, history_clip_origin.y));
                history.scroll.reflectScrolledClipView(&history_clip);
            }
            let scroll_bounds = history.scroll.bounds();
            let center_x = scroll_bounds.size.width / 2.0;
            history.status.setAlignment(NSTextAlignment::Center);
            history.status.setFrame(NSRect::new(
                NSPoint::new(12.0, scroll_bounds.size.height / 2.0 - 32.0),
                NSSize::new((scroll_bounds.size.width - 24.0).max(1.0), 20.0),
            ));
            history.loading_spinner.setFrameOrigin(NSPoint::new(
                center_x - history.loading_spinner.frame().size.width / 2.0,
                scroll_bounds.size.height / 2.0 + 2.0,
            ));
        }
        if let Some(files) = self.ivars().files.get() {
            files.sidebar_root.setFrame(sidebar_bounds);
            let files_safe = files.sidebar_root.safeAreaInsets();
            let files_top = sidebar_bounds.size.height - files_safe.top;
            let search_visible = self.ivars().files_search_visible.get();
            files.search.setHidden(!search_visible);
            files.search.setFrame(NSRect::new(
                NSPoint::new(12.0, (files_top - 38.0).max(files_safe.bottom + 52.0)),
                NSSize::new((sidebar_bounds.size.width - 24.0).max(1.0), 30.0),
            ));
            let files_scroll_top = if search_visible {
                files_top - 38.0
            } else {
                sidebar_bounds.size.height
            };
            files.scroll.setFrame(NSRect::new(
                NSPoint::new(8.0, files_safe.bottom),
                NSSize::new(
                    (sidebar_bounds.size.width - 16.0).max(1.0),
                    (files_scroll_top - files_safe.bottom).max(1.0),
                ),
            ));
            let content_width = files.scroll.contentSize().width.max(1.0);
            let table_frame = files.table.frame();
            files
                .table
                .setFrameSize(NSSize::new(content_width, table_frame.size.height));
            if let Some(column) = files.table.tableColumns().firstObject() {
                column.setWidth(content_width);
            }
        }
        if let Some(containers) = self.ivars().containers.get() {
            containers.sidebar_root.setFrame(sidebar_bounds);
            let containers_safe = containers.sidebar_root.safeAreaInsets();
            let containers_top = sidebar_bounds.size.height - containers_safe.top;
            let search_visible = self.ivars().containers_search_visible.get();
            containers.search.setHidden(!search_visible);
            containers.search.setFrame(NSRect::new(
                NSPoint::new(
                    12.0,
                    (containers_top - 38.0).max(containers_safe.bottom + 52.0),
                ),
                NSSize::new((sidebar_bounds.size.width - 24.0).max(1.0), 30.0),
            ));
            let containers_scroll_top = if search_visible {
                containers_top - 38.0
            } else {
                sidebar_bounds.size.height
            };
            containers.scroll.setFrame(NSRect::new(
                NSPoint::new(8.0, containers_safe.bottom),
                NSSize::new(
                    (sidebar_bounds.size.width - 16.0).max(1.0),
                    (containers_scroll_top - containers_safe.bottom).max(1.0),
                ),
            ));
            let content_width = containers.scroll.contentSize().width.max(1.0);
            let table_frame = containers.table.frame();
            containers
                .table
                .setFrameSize(NSSize::new(content_width, table_frame.size.height));
            if let Some(column) = containers.table.tableColumns().firstObject() {
                column.setWidth(
                    (content_width - CONTAINER_SOURCE_LIST_HORIZONTAL_INSET * 2.0).max(1.0),
                );
            }
        }
        if let Some(agents) = self.ivars().agents.get() {
            agents.sidebar_root.setFrame(sidebar_bounds);
            let agents_safe = agents.sidebar_root.safeAreaInsets();
            let agents_top = sidebar_bounds.size.height - agents_safe.top;
            let launch_widths = [68.0, 104.0, 62.0];
            let launch_spacing = 8.0;
            let launch_total = launch_widths.iter().sum::<f64>() + launch_spacing * 2.0;
            let mut launch_x = ((sidebar_bounds.size.width - launch_total) / 2.0).max(8.0);
            for (button, width) in [
                (&agents.new_chat, launch_widths[0]),
                (&agents.codex_cli, launch_widths[1]),
                (&agents.agy, launch_widths[2]),
            ] {
                button.setFrame(NSRect::new(
                    NSPoint::new(launch_x, agents_safe.bottom + 12.0),
                    NSSize::new(width, 32.0),
                ));
                launch_x += width + launch_spacing;
            }
            let search_visible = self.ivars().agents_search_visible.get();
            agents.history_search.setHidden(!search_visible);
            agents.history_scope.setHidden(!search_visible);
            agents.history_search.setFrame(NSRect::new(
                NSPoint::new(14.0, (agents_top - 38.0).max(agents_safe.bottom + 54.0)),
                NSSize::new((sidebar_bounds.size.width - 112.0).max(1.0), 28.0),
            ));
            agents.history_scope.setFrame(NSRect::new(
                NSPoint::new(
                    (sidebar_bounds.size.width - 92.0).max(14.0),
                    (agents_top - 38.0).max(agents_safe.bottom + 54.0),
                ),
                NSSize::new(78.0, 28.0),
            ));
            let threads_top = if search_visible {
                agents_top - 38.0
            } else {
                agents_top
            };
            agents.threads_scroll.setFrame(NSRect::new(
                NSPoint::new(8.0, agents_safe.bottom + 54.0),
                NSSize::new(
                    (sidebar_bounds.size.width - 16.0).max(1.0),
                    (threads_top - agents_safe.bottom - 54.0).max(1.0),
                ),
            ));
            self.refresh_native_agent_thread_rows();
        }
    }

    fn show_changes_search(&self) {
        self.ivars().changes_search_visible.set(true);
        self.layout_sidebar();
        let (Some(window), Some(search)) =
            (self.ivars().window.get(), self.ivars().changes_search.get())
        else {
            return;
        };
        window.makeFirstResponder(Some(search));
    }

    fn hide_changes_search(&self) {
        self.ivars().changes_search_visible.set(false);
        self.ivars().changes_filter_query.borrow_mut().clear();
        if let Some(search) = self.ivars().changes_search.get() {
            search.setStringValue(&NSString::new());
        }
        self.refresh_changed_file_results();
        self.layout_sidebar();
        if let Some(window) = self.ivars().window.get() {
            window.makeFirstResponder(None);
        }
    }

    fn show_history_search(&self) {
        self.ivars().history_search_visible.set(true);
        self.layout_sidebar();
        let (Some(window), Some(history)) = (self.ivars().window.get(), self.ivars().history.get())
        else {
            return;
        };
        window.makeFirstResponder(Some(&history.search));
    }

    fn hide_history_search(&self) {
        self.ivars().history_search_visible.set(false);
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        history.search.setStringValue(&NSString::new());
        history.query.borrow_mut().clear();
        if history.loading.get() {
            history.pending_search.set(true);
        } else {
            self.request_history_page(true);
        }
        self.layout_sidebar();
        if let Some(window) = self.ivars().window.get() {
            window.makeFirstResponder(None);
        }
    }

    fn show_files_search(&self) {
        self.ivars().files_search_visible.set(true);
        self.layout_sidebar();
        let (Some(window), Some(files)) = (self.ivars().window.get(), self.ivars().files.get())
        else {
            return;
        };
        window.makeFirstResponder(Some(&files.search));
        log::debug!("native Files search shown and focused");
    }

    fn hide_files_search(&self) {
        self.ivars().files_search_visible.set(false);
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.search.setStringValue(&NSString::new());
        files.query.borrow_mut().clear();
        files.table.reloadData();
        files.status.setHidden(true);
        self.layout_sidebar();
        log::debug!("native Files search dismissed with provider lifecycle");
    }

    fn show_editor_search(&self) {
        let (Some(window), Some(files)) = (self.ivars().window.get(), self.ivars().files.get())
        else {
            return;
        };
        files.editor_search_visible.set(true);
        files.editor_search_panel.setHidden(false);
        self.layout_files_preview();
        window.makeFirstResponder(Some(&files.editor_search));
        self.update_editor_search_status();
        log::debug!("native Skia editor search shown and focused");
    }

    fn hide_editor_search(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.editor_search_visible.set(false);
        files.editor_search.setStringValue(&NSString::new());
        files.editor_search_panel.setHidden(true);
        files.preview_code.clear_search();
        self.layout_files_preview();
        if let Some(window) = self.ivars().window.get() {
            window.makeFirstResponder(Some(&files.preview_code));
        }
        log::debug!("native Skia editor search dismissed");
    }

    fn update_editor_search_status(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let (active, total) = files.preview_code.search_status();
        files
            .editor_search_status
            .setStringValue(&NSString::from_str(&active.map_or_else(
                || format!("0 of {total}"),
                |active| format!("{active} of {total}"),
            )));
    }

    fn show_containers_search(&self) {
        self.ivars().containers_search_visible.set(true);
        self.layout_sidebar();
        let (Some(window), Some(containers)) =
            (self.ivars().window.get(), self.ivars().containers.get())
        else {
            return;
        };
        window.makeFirstResponder(Some(&containers.search));
        log::debug!("native Containers search shown and focused");
    }

    fn hide_containers_search(&self) {
        self.ivars().containers_search_visible.set(false);
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        containers.search.setStringValue(&NSString::new());
        containers.query.borrow_mut().clear();
        containers.table.reloadData();
        if !containers.rows.borrow().is_empty() {
            containers.status.setHidden(true);
            containers.scroll.setHidden(false);
        } else {
            containers.scroll.setHidden(true);
        }
        self.layout_sidebar();
        log::debug!("native Containers search dismissed with provider lifecycle");
    }

    fn show_agents_search(&self) {
        self.ivars().agents_search_visible.set(true);
        self.layout_sidebar();
        let (Some(window), Some(agents)) = (self.ivars().window.get(), self.ivars().agents.get())
        else {
            return;
        };
        window.makeFirstResponder(Some(&agents.history_search));
        log::debug!("native Agents search shown and focused");
    }

    fn hide_agents_search(&self) {
        self.ivars().agents_search_visible.set(false);
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents.history_search.setStringValue(&NSString::new());
        self.refresh_native_agent_thread_rows();
        self.layout_sidebar();
        if let Some(window) = self.ivars().window.get() {
            window.makeFirstResponder(None);
        }
        log::debug!("native Agents search dismissed with provider lifecycle");
    }

    fn clear_changed_file_preview(&self, message: &str) {
        self.ivars().selected_change_path.borrow_mut().take();
        self.ivars().loaded_diff_path.borrow_mut().take();
        self.ivars().loaded_image_path.borrow_mut().take();
        self.ivars().diff_loading_request_id.set(None);
        self.ivars()
            .diff_request_id
            .set(self.ivars().diff_request_id.get().wrapping_add(1));
        if let Some(diff_view) = self.ivars().diff_view.get() {
            diff_view.clear();
            diff_view.setHidden(true);
        }
        if let Some(image) = self.ivars().image_preview.get() {
            image.setImage(None);
            image.setHidden(true);
        }
        self.clear_changed_binary_preview();
        if let Some(spinner) = self.ivars().diff_spinner.get() {
            unsafe { spinner.stopAnimation(None) };
            spinner.setHidden(true);
        }
        if self.is_active_page("changes")
            && let Some(empty) = self.ivars().content_empty.get()
        {
            empty.setStringValue(&NSString::from_str(message));
            empty.setHidden(false);
        }
    }

    fn set_all_visible_changed_files_checked(&self, active: bool) {
        let Some(snapshot) = self.ivars().repository_snapshot.borrow().as_ref().cloned() else {
            return;
        };
        let query = self.ivars().changes_filter_query.borrow().clone();
        let mut checked = self.ivars().checked_change_paths.borrow_mut();
        for file in snapshot
            .changed_files
            .into_iter()
            .filter(|file| changed_file_matches_query(&file.path, &file.status, &query))
        {
            if active {
                checked.insert(file.path);
            } else {
                checked.remove(&file.path);
            }
        }
        drop(checked);
        self.refresh_changed_file_results();
        self.update_commit_composer_state();
    }

    fn changed_file_path_for_tag(&self, tag: isize) -> Option<String> {
        let index = usize::try_from(tag).ok()?;
        self.ivars()
            .repository_snapshot
            .borrow()
            .as_ref()?
            .changed_files
            .get(index)
            .map(|file| file.path.clone())
    }

    fn local_changed_file_path(&self, relative_path: &str) -> Result<PathBuf, String> {
        let workspace_id = self
            .ivars()
            .active_workspace_id
            .borrow()
            .clone()
            .ok_or_else(|| "No workspace is active.".to_string())?;
        let workspaces = self.ivars().workspaces.borrow();
        let workspace = workspaces
            .iter()
            .find(|workspace| workspace.selection_id() == workspace_id)
            .ok_or_else(|| "The active workspace is no longer available.".to_string())?;
        if !matches!(
            workspace.workspace.provider,
            craic_config::WorkspaceProvider::Local
        ) {
            return Err("Finder actions are unavailable for SSH workspaces.".to_string());
        }
        let root = craic_config::expand_config_path_for_ui(&workspace.workspace.path)
            .unwrap_or_else(|| PathBuf::from(&workspace.workspace.path));
        Ok(root.join(relative_path))
    }

    fn copy_text_to_pasteboard(&self, text: &str) {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        if !pasteboard
            .setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
        {
            self.present_path_action_error(
                "Unable to Copy Path",
                "The path could not be written to the pasteboard.",
            );
        }
    }

    fn store_workspace_file_clipboard(&self, move_item: bool) {
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.path.is_root()
            || if move_item {
                !info.capabilities.movable
            } else {
                !info.capabilities.readable
            }
        {
            return;
        }
        let payload = format!(
            "{}\n{}\n{}",
            access.workspace().id.as_str(),
            if move_item { "move" } else { "copy" },
            info.path.display()
        );
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        let stored_payload = pasteboard.setString_forType(
            &NSString::from_str(&payload),
            &workspace_file_clipboard_type(),
        );
        let stored_text = pasteboard
            .setString_forType(&NSString::from_str(&access.copy_path(&info.path)), unsafe {
                NSPasteboardTypeString
            });
        if !stored_payload || !stored_text {
            self.present_path_action_error(
                "Unable to Copy Item",
                "The workspace item could not be written to the pasteboard.",
            );
        }
    }

    fn present_path_action_error(&self, heading: &str, message: &str) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(heading));
        alert.setInformativeText(&NSString::from_str(message));
        alert.addButtonWithTitle(&NSString::from_str("OK"));
        alert.beginSheetModalForWindow_completionHandler(window, None);
    }

    fn active_local_workspace_path(&self) -> Result<(String, PathBuf), String> {
        let workspace_id = self
            .ivars()
            .active_workspace_id
            .borrow()
            .clone()
            .ok_or_else(|| "Open a workspace before using Quick Actions.".to_string())?;
        let workspaces = self.ivars().workspaces.borrow();
        let workspace = workspaces
            .iter()
            .find(|workspace| workspace.selection_id() == workspace_id)
            .ok_or_else(|| "The active workspace is no longer available.".to_string())?;
        if !matches!(
            workspace.workspace.provider,
            craic_config::WorkspaceProvider::Local
        ) {
            return Err(
                "Project Quick Actions are available only for local workspaces.".to_string(),
            );
        }
        let path = craic_config::expand_config_path_for_ui(&workspace.workspace.path)
            .unwrap_or_else(|| PathBuf::from(&workspace.workspace.path));
        Ok((workspace_id, path))
    }

    fn open_active_repository_in_editor(&self) {
        let (_, path) = match self.active_local_workspace_path() {
            Ok(workspace) => workspace,
            Err(message) => {
                self.present_path_action_error("Unable to Open Repository", &message);
                return;
            }
        };
        let workspace = NSWorkspace::sharedWorkspace();
        let Some(application_url) = workspace
            .URLForApplicationWithBundleIdentifier(&NSString::from_str("com.microsoft.VSCode"))
        else {
            self.present_path_action_error(
                "Unable to Open Repository",
                "Visual Studio Code is not installed or is not registered with macOS.",
            );
            return;
        };
        let repository_url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let configuration = NSWorkspaceOpenConfiguration::new();
        configuration.setActivates(true);
        workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
            &NSArray::from_slice(&[&*repository_url]),
            &application_url,
            &configuration,
            None,
        );
    }

    fn open_active_repository_in_ghostty(&self) {
        let (_, path) = match self.active_local_workspace_path() {
            Ok(workspace) => workspace,
            Err(message) => {
                self.present_path_action_error("Unable to Open Ghostty", &message);
                return;
            }
        };
        let workspace = NSWorkspace::sharedWorkspace();
        let Some(application_url) = workspace
            .URLForApplicationWithBundleIdentifier(&NSString::from_str("com.mitchellh.ghostty"))
        else {
            self.present_path_action_error(
                "Unable to Open Ghostty",
                "Ghostty is not installed or is not registered with macOS.",
            );
            return;
        };
        let working_directory = NSString::from_str("--working-directory");
        let path = NSString::from_str(&path.to_string_lossy());
        let arguments = NSArray::from_slice(&[&*working_directory, &*path]);
        let configuration = NSWorkspaceOpenConfiguration::new();
        configuration.setActivates(true);
        configuration.setArguments(&arguments);
        workspace.openApplicationAtURL_configuration_completionHandler(
            &application_url,
            &configuration,
            None,
        );
    }

    fn show_active_repository_in_finder(&self) {
        let (_, path) = match self.active_local_workspace_path() {
            Ok(workspace) => workspace,
            Err(message) => {
                self.present_path_action_error("Unable to Open Finder", &message);
                return;
            }
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        if !NSWorkspace::sharedWorkspace().openURL(&url) {
            self.present_path_action_error(
                "Unable to Open Finder",
                &format!("Finder could not open {}.", path.display()),
            );
        }
    }

    fn open_active_repository_remote(&self) {
        let Some(remote_url) = self
            .ivars()
            .repository_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.remote_url.clone())
        else {
            self.present_path_action_error(
                "No Remote Repository",
                "The active repository does not have a remote URL.",
            );
            return;
        };
        let web_url = craic_vcs::git::remote_web_url(&remote_url);
        let Some(url) = NSURL::URLWithString(&NSString::from_str(&web_url)) else {
            self.present_path_action_error(
                "Unable to Open Remote",
                "The repository remote URL is invalid.",
            );
            return;
        };
        if !NSWorkspace::sharedWorkspace().openURL(&url) {
            self.present_path_action_error(
                "Unable to Open Remote",
                "The repository remote URL could not be opened.",
            );
        }
    }

    fn initialize_active_repository(&self) {
        if self.ivars().repository_initialization_in_progress.get() {
            return;
        }
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            self.present_path_action_error(
                "Initialize Repository Failed",
                "The active workspace is unavailable.",
            );
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        self.ivars().repository_initialization_in_progress.set(true);
        if let Some(button) = self.ivars().content_home_initialize.get() {
            button.setTitle(&NSString::from_str("Initializing…"));
            button.setEnabled(false);
        }
        if let Err(error) = requests.try_send(RepositoryRequest::InitializeRepository {
            workspace_id,
            handle,
            cancellation,
        }) {
            self.ivars()
                .repository_initialization_in_progress
                .set(false);
            if let Some(button) = self.ivars().content_home_initialize.get() {
                button.setTitle(&NSString::from_str("Initialize"));
                button.setEnabled(true);
            }
            self.present_path_action_error(
                "Initialize Repository Failed",
                &format!("The initialization request could not be queued: {error}"),
            );
        }
    }

}
