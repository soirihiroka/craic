impl AppDelegate {
    fn request_history_avatar(&self, email: &str) {
        let email = email.trim();
        if email.is_empty() {
            return;
        }
        let cache_key = format!("email:{}", email.to_ascii_lowercase());
        self.request_avatar(cache_key, AvatarSource::Email(email.to_string()));
    }

    fn request_avatar(&self, cache_key: String, source: AvatarSource) {
        if self.ivars().avatar_images.borrow().contains_key(&cache_key)
            || !self
                .ivars()
                .avatar_in_flight
                .borrow_mut()
                .insert(cache_key.clone())
        {
            return;
        }
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.ivars()
                .avatar_in_flight
                .borrow_mut()
                .remove(&cache_key);
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            self.ivars()
                .avatar_in_flight
                .borrow_mut()
                .remove(&cache_key);
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadAvatar {
            cache_key: cache_key.clone(),
            source,
            handle,
        }) {
            self.ivars()
                .avatar_in_flight
                .borrow_mut()
                .remove(&cache_key);
            log::debug!("avatar request coalesced key={cache_key} error={error}");
        }
    }

    fn apply_avatar(&self, cache_key: &str, result: Result<Vec<u8>, String>) {
        self.ivars().avatar_in_flight.borrow_mut().remove(cache_key);
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => {
                log::debug!("avatar unavailable key={cache_key} error={error}");
                return;
            }
        };
        let data = NSData::with_bytes(&bytes);
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
            log::debug!("avatar decode failed key={cache_key}");
            return;
        };
        self.ivars()
            .avatar_images
            .borrow_mut()
            .insert(cache_key.to_string(), image.clone());
        if let Some(history) = self.ivars().history.get() {
            history.table.reloadData();
            if history.avatar_source.borrow().as_deref() == Some(cache_key) {
                history.avatar.setImage(Some(&image));
                history.avatar.setHidden(false);
            }
        }
        if self.ivars().commit_avatar_source.borrow().as_deref() == Some(cache_key)
            && let Some(composer) = self.ivars().commit_composer.get()
        {
            let snapshot = self.ivars().repository_snapshot.borrow();
            let warning = snapshot
                .as_ref()
                .and_then(RepositorySnapshot::remote_author_warning_text);
            composer.set_author(
                snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.user_name.as_deref()),
                Some(&image),
                warning.as_deref(),
            );
            log::debug!("native commit author avatar applied key={cache_key}");
        }
        if self
            .ivars()
            .author_popover
            .get()
            .is_some_and(|popover| popover.isShown())
            && let Some(table) = self.ivars().author_table.get()
        {
            table.reloadData();
        }
    }

    fn reset_history(&self, message: &str) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        history
            .generation
            .set(history.generation.get().wrapping_add(1));
        history.commits.borrow_mut().clear();
        history.files.borrow_mut().clear();
        history.cursor.borrow_mut().take();
        history.selected_hash.borrow_mut().take();
        history.selected_commit.borrow_mut().take();
        history.selected_parent_hash.borrow_mut().take();
        history.parent_loaded.set(false);
        history.pending_checkout_parent.set(false);
        history.pending_amend.set(false);
        history.detail_loading.set(false);
        history.selected_file.borrow_mut().take();
        history.loaded_diff_path.borrow_mut().take();
        history.loaded_binary_path.borrow_mut().take();
        history.preview_cache.borrow_mut().clear();
        history.has_more.set(false);
        history.loading.set(false);
        history.action_in_progress.set(false);
        history.table.reloadData();
        // SAFETY: The nil sender only clears this table's native selection state.
        unsafe { history.table.deselectAll(None) };
        history.files_table.reloadData();
        // SAFETY: The nil sender only clears the native file-table selection.
        unsafe { history.files_table.deselectAll(None) };
        history.diff.clear();
        history.diff.setHidden(true);
        self.clear_history_binary_preview();
        history
            .title
            .setStringValue(&NSString::from_str("Select a commit"));
        history.avatar.setHidden(true);
        history.avatar_source.borrow_mut().take();
        history.metadata.setStringValue(&NSString::new());
        history.added.setHidden(true);
        history.deleted.setHidden(true);
        history.comment.setStringValue(&NSString::new());
        history
            .file_count
            .setStringValue(&NSString::from_str("No commit selected"));
        history.copy_hash.setEnabled(false);
        history.open_remote.setEnabled(false);
        history.empty.setStringValue(&NSString::from_str(message));
        history.empty.setHidden(false);
        history.status.setStringValue(&NSString::from_str(message));
        history.status.setHidden(false);
        // SAFETY: The spinner is retained by HistoryUi and only driven on AppKit's main thread.
        if message.starts_with("Loading") {
            unsafe { history.loading_spinner.startAnimation(None) };
        } else {
            unsafe { history.loading_spinner.stopAnimation(None) };
        }
    }

    fn clear_history_binary_preview(&self) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        history.binary_preview.setHidden(true);
        let subviews = history.binary_preview.subviews();
        for index in (0..subviews.count()).rev() {
            subviews.objectAtIndex(index).removeFromSuperview();
        }
        history.binary_font_registrations.borrow_mut().clear();
    }

    fn selected_history_hash(&self) -> Option<String> {
        self.ivars()
            .history
            .get()
            .and_then(|history| history.selected_hash.borrow().clone())
    }

    fn retry_selected_history_commit_detail(&self) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        let Some(hash) = history.selected_hash.borrow().clone() else {
            return;
        };
        let Some(index) = history
            .commits
            .borrow()
            .iter()
            .position(|commit| commit.hash == hash)
        else {
            return;
        };
        history.selected_hash.borrow_mut().take();
        self.select_history_commit(index);
    }

    fn rebuild_history_menu(&self) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        history.menu.removeAllItems();
        if history.selected_hash.borrow().is_none() {
            return;
        }
        let idle = !history.action_in_progress.get();
        let add = |title: &str, action: Sel, enabled: bool| {
            let item = unsafe {
                history.menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::new(),
                )
            };
            unsafe { item.setTarget(Some(self)) };
            item.setEnabled(enabled);
        };
        add("Checkout Commit", sel!(checkoutHistoryCommit:), idle);
        add(
            "Checkout Parent",
            sel!(checkoutHistoryParent:),
            idle && (!history.parent_loaded.get()
                || history.selected_parent_hash.borrow().is_some()),
        );
        history.menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        add("New Branch Here…", sel!(newHistoryBranch:), idle);
        add("Create Tag…", sel!(createHistoryTag:), idle);
        history.menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        add("Cherry-Pick Commit", sel!(cherryPickHistoryCommit:), idle);
        add("Revert Commit…", sel!(revertHistoryCommit:), idle);
        history.menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        add(
            "Amend HEAD With This Message…",
            sel!(amendHistoryHead:),
            idle,
        );
        add(
            "Reset Current Branch Here (--mixed)…",
            sel!(resetHistoryMixed:),
            idle,
        );
        add(
            "Reset Current Branch Here (--hard)…",
            sel!(resetHistoryHard:),
            idle,
        );
        history.menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        let has_remote = self
            .ivars()
            .repository_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.remote_url.as_ref())
            .is_some();
        add(
            "Open Commit on Remote",
            sel!(openHistoryRemote:),
            idle && has_remote,
        );
    }

    fn run_history_action(&self, action: HistoryAction) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        if history.action_in_progress.replace(true) {
            return;
        }
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            history.action_in_progress.set(false);
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            history.action_in_progress.set(false);
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            history.action_in_progress.set(false);
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            history.action_in_progress.set(false);
            return;
        };
        history
            .status
            .setStringValue(&NSString::from_str("Working…"));
        history.status.setHidden(false);
        if let Err(error) = requests.try_send(RepositoryRequest::RunHistoryAction {
            workspace_id,
            handle,
            action,
            cancellation,
        }) {
            history.action_in_progress.set(false);
            history.status.setHidden(true);
            self.present_path_action_error(
                "History Action Failed",
                &format!("The history action could not be queued: {error}"),
            );
        }
    }

    fn show_history_name_sheet(&self, branch: bool) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let Some(hash) = self.selected_history_hash() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(if branch {
            "New Branch"
        } else {
            "Create Tag"
        }));
        alert.setInformativeText(&NSString::from_str(if branch {
            "Enter a branch name for this commit."
        } else {
            "Enter a tag name for this commit."
        }));
        alert.addButtonWithTitle(&NSString::from_str(if branch {
            "Create Branch"
        } else {
            "Create Tag"
        }));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let field = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(320.0, 26.0)),
        );
        field.setPlaceholderString(Some(&NSString::from_str(if branch {
            "Branch name"
        } else {
            "Tag name"
        })));
        alert.setAccessoryView(Some(&field));
        let delegate = self.retain();
        let field_for_completion = field.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let name = field_for_completion.stringValue().to_string();
            let name = name.trim();
            if name.is_empty() {
                delegate.present_path_action_error(
                    if branch {
                        "Create Branch Failed"
                    } else {
                        "Create Tag Failed"
                    },
                    if branch {
                        "Enter a branch name."
                    } else {
                        "Enter a tag name."
                    },
                );
                return;
            }
            delegate.run_history_action(if branch {
                HistoryAction::CreateBranch {
                    branch: name.to_string(),
                    hash: hash.clone(),
                }
            } else {
                HistoryAction::CreateTag {
                    tag: name.to_string(),
                    hash: hash.clone(),
                }
            });
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        alert.window().makeFirstResponder(Some(&field));
    }

    fn confirm_history_action(&self, mode: ResetMode, revert: bool) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let Some(hash) = self.selected_history_hash() else {
            return;
        };
        let (heading, body, button) = if revert {
            (
                "Revert Commit",
                "This creates a new commit that reverses the selected commit.",
                "Revert Commit",
            )
        } else if mode == ResetMode::Hard {
            (
                "Hard Reset Current Branch",
                "This moves the current branch to the selected commit and discards working tree changes.",
                "Reset --hard",
            )
        } else {
            (
                "Reset Current Branch",
                "This moves the current branch to the selected commit and leaves file changes in the working tree.",
                "Reset --mixed",
            )
        };
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(heading));
        alert.setInformativeText(&NSString::from_str(body));
        alert
            .addButtonWithTitle(&NSString::from_str(button))
            .setHasDestructiveAction(true);
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            delegate.run_history_action(if revert {
                HistoryAction::Revert(hash.clone())
            } else {
                HistoryAction::Reset {
                    hash: hash.clone(),
                    mode,
                }
            });
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn show_history_amend_sheet(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let Some(commit) = self
            .ivars()
            .history
            .get()
            .and_then(|history| history.selected_commit.borrow().clone())
        else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str("Amend HEAD"));
        alert.setInformativeText(&NSString::from_str(
            "Edit the message for HEAD. The selected commit message is used as the starting point.",
        ));
        let root = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 190.0)),
        );
        let summary = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::new(0.0, 158.0), NSSize::new(420.0, 26.0)),
        );
        summary.setPlaceholderString(Some(&NSString::from_str("Summary")));
        summary.setStringValue(&NSString::from_str(&commit.subject));
        root.addSubview(&summary);
        let description = NSTextView::initWithFrame(
            NSTextView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 146.0)),
        );
        description.setEditable(true);
        description.setSelectable(true);
        description.setRichText(false);
        description.setString(&NSString::from_str(&commit.comment));
        description.setTextContainerInset(NSSize::new(8.0, 8.0));
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(420.0, 146.0)),
        );
        scroll.setBorderType(NSBorderType::BezelBorder);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&description));
        root.addSubview(&scroll);
        alert.setAccessoryView(Some(&root));
        alert
            .addButtonWithTitle(&NSString::from_str("Amend HEAD"))
            .setHasDestructiveAction(true);
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let delegate = self.retain();
        let completion_summary = summary.clone();
        let completion_description = description.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            delegate.run_history_action(HistoryAction::Amend {
                summary: completion_summary
                    .stringValue()
                    .to_string()
                    .trim()
                    .to_string(),
                description: completion_description.string().to_string(),
            });
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        alert.window().makeFirstResponder(Some(&summary));
    }

    fn request_history_page(&self, reset: bool) {
        let Some(history) = self.ivars().history.get() else {
            self.complete_pending_page_service(
                "history",
                Err("The History page is unavailable".to_string()),
            );
            return;
        };
        if history.loading.get() {
            self.complete_pending_page_service(
                "history",
                Err("History is already refreshing".to_string()),
            );
            return;
        }
        if reset {
            history
                .generation
                .set(history.generation.get().wrapping_add(1));
            history.commits.borrow_mut().clear();
            history.cursor.borrow_mut().take();
            history.selected_hash.borrow_mut().take();
            history.selected_commit.borrow_mut().take();
            history.selected_parent_hash.borrow_mut().take();
            history.parent_loaded.set(false);
            history.pending_checkout_parent.set(false);
            history.pending_amend.set(false);
            history.detail_loading.set(false);
            history.selected_file.borrow_mut().take();
            history.loaded_diff_path.borrow_mut().take();
            history.loaded_binary_path.borrow_mut().take();
            history.preview_cache.borrow_mut().clear();
            history.table.reloadData();
            history.diff.clear();
            history.diff.setHidden(true);
            self.clear_history_binary_preview();
            history
                .empty
                .setStringValue(&NSString::from_str("Loading commits..."));
            history.empty.setHidden(false);
        } else if !history.has_more.get() {
            return;
        }
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            history
                .status
                .setStringValue(&NSString::from_str("No workspace"));
            self.complete_pending_page_service(
                "history",
                Err("No workspace is active".to_string()),
            );
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            history
                .status
                .setStringValue(&NSString::from_str("No Git repository"));
            self.complete_pending_page_service(
                "history",
                Err("The workspace is not a Git repository".to_string()),
            );
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            self.complete_pending_page_service(
                "history",
                Err("The workspace is shutting down".to_string()),
            );
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            history
                .status
                .setStringValue(&NSString::from_str("History service unavailable"));
            self.complete_pending_page_service(
                "history",
                Err("The history service is unavailable".to_string()),
            );
            return;
        };
        history.loading.set(true);
        self.set_page_badge("history", NativePageBadge::Indicator);
        let empty = history.commits.borrow().is_empty();
        let search_active = !history.query.borrow().is_empty();
        if empty {
            history
                .loading_spinner
                .setControlSize(NSControlSize::Regular);
            history
                .loading_spinner
                .setFrameSize(NSSize::new(24.0, 24.0));
            // SAFETY: The retained empty-state spinner is driven on AppKit's main thread.
            unsafe { history.loading_spinner.startAnimation(None) };
            history
                .status
                .setStringValue(&NSString::from_str(if search_active {
                    "Searching commits..."
                } else {
                    "Loading commits..."
                }));
            history.status.setHidden(false);
        } else {
            // Pagination is represented by a real final table row so the indicator scrolls
            // with the commit cards instead of floating over the viewport.
            unsafe { history.loading_spinner.stopAnimation(None) };
            history.status.setHidden(true);
            history.table.reloadData();
        }
        self.layout_sidebar();
        let request = RepositoryRequest::LoadHistoryPage {
            workspace_id,
            handle,
            query: history.query.borrow().clone(),
            after: history.cursor.borrow().clone(),
            generation: history.generation.get(),
            cancellation,
        };
        if let Err(error) = requests.try_send(request) {
            history.loading.set(false);
            // SAFETY: The retained spinner is driven on AppKit's main thread.
            unsafe { history.loading_spinner.stopAnimation(None) };
            history.table.reloadData();
            history.status.setHidden(false);
            history
                .status
                .setStringValue(&NSString::from_str("Unable to queue history load"));
            self.layout_sidebar();
            self.set_page_badge("history", NativePageBadge::None);
            log::warn!("history page queue rejected request error={error}");
            self.complete_pending_page_service("history", Err(error.to_string()));
        }
    }

    fn apply_history_page(
        &self,
        workspace_id: &str,
        generation: u64,
        result: Result<CommitPage, String>,
    ) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || history.generation.get() != generation
        {
            log::debug!("discarding stale history page workspace={workspace_id}");
            return;
        }
        let page_service_result = result
            .as_ref()
            .map(|page| serde_json::json!({ "commits": page.commits.len() }))
            .map_err(Clone::clone);
        history.loading.set(false);
        self.set_page_badge("history", NativePageBadge::None);
        // SAFETY: The retained spinner is driven on AppKit's main thread.
        unsafe { history.loading_spinner.stopAnimation(None) };
        if history.pending_search.replace(false) {
            self.request_history_page(true);
            return;
        }
        match result {
            Ok(page) => {
                history.has_more.set(page.has_more);
                let mut commits = history.commits.borrow_mut();
                for commit in page.commits {
                    if !commits
                        .iter()
                        .any(|candidate| candidate.hash == commit.hash)
                    {
                        commits.push(commit);
                    }
                }
                history
                    .cursor
                    .replace(commits.last().map(|commit| commit.hash.clone()));
                let count = commits.len();
                drop(commits);
                history.table.reloadData();
                if count == 0 {
                    history.status.setHidden(false);
                    history.status.setStringValue(&NSString::from_str(
                        if history.query.borrow().is_empty() {
                            "History is empty."
                        } else {
                            "No matching commits."
                        },
                    ));
                } else if history.query.borrow().is_empty() {
                    history.status.setHidden(true);
                } else {
                    history.status.setHidden(false);
                    history.status.setStringValue(&NSString::from_str(
                        if history.has_more.get() {
                            format!("{count} loaded, more available")
                        } else {
                            format!("{count} matches")
                        }
                        .as_str(),
                    ));
                }
                if count > 0 && history.selected_hash.borrow().is_none() {
                    let first = NSIndexSet::indexSetWithIndex(0);
                    history
                        .table
                        .selectRowIndexes_byExtendingSelection(&first, false);
                    self.select_history_commit(0);
                }
            }
            Err(error) => {
                history.has_more.set(false);
                history.status.setHidden(false);
                history.status.setStringValue(&NSString::from_str(&error));
                history
                    .empty
                    .setStringValue(&NSString::from_str("Unable to load history"));
                history.empty.setHidden(false);
                log::warn!("native history page failed workspace={workspace_id}: {error}");
            }
        }
        self.layout_sidebar();
        self.complete_pending_page_service("history", page_service_result);
    }

    fn select_history_commit(&self, index: usize) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        let Some(commit) = history.commits.borrow().get(index).cloned() else {
            return;
        };
        if history.selected_hash.borrow().as_deref() == Some(commit.hash.as_str()) {
            return;
        }
        history.selected_hash.replace(Some(commit.hash.clone()));
        history.selected_commit.borrow_mut().take();
        history.selected_parent_hash.borrow_mut().take();
        history.parent_loaded.set(false);
        history.pending_checkout_parent.set(false);
        history.pending_amend.set(false);
        history.detail_loading.set(true);
        history.selected_file.borrow_mut().take();
        history.loaded_diff_path.borrow_mut().take();
        history.loaded_binary_path.borrow_mut().take();
        history.files.borrow_mut().clear();
        history
            .detail_request_id
            .set(history.detail_request_id.get().wrapping_add(1));
        history
            .title
            .setStringValue(&NSString::from_str(&commit.subject));
        history
            .metadata
            .setStringValue(&NSString::from_str("Loading commit details…"));
        history.avatar.setHidden(true);
        history.avatar_source.borrow_mut().take();
        history.added.setHidden(true);
        history.deleted.setHidden(true);
        history.comment.setStringValue(&NSString::new());
        history
            .file_count
            .setStringValue(&NSString::from_str("Loading changed files…"));
        history.files_table.reloadData();
        history.copy_hash.setEnabled(false);
        history.open_remote.setEnabled(false);
        history.diff.clear();
        history.diff.setHidden(true);
        self.clear_history_binary_preview();
        history
            .empty
            .setStringValue(&NSString::from_str("Loading commit…"));
        history.empty.setHidden(false);
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
        let request = RepositoryRequest::LoadHistoryCommit {
            workspace_id,
            handle,
            hash: commit.hash,
            request_id: history.detail_request_id.get(),
            cancellation,
        };
        if let Err(error) = requests.try_send(request) {
            history.detail_loading.set(false);
            history
                .empty
                .setStringValue(&NSString::from_str("Unable to queue commit details"));
            log::warn!("history detail queue rejected request error={error}");
        }
    }

    fn apply_history_commit(
        &self,
        workspace_id: &str,
        hash: &str,
        request_id: u64,
        result: Result<(Commit, Vec<ChangedFile>, Option<String>, bool), String>,
    ) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || history.detail_request_id.get() != request_id
            || history.selected_hash.borrow().as_deref() != Some(hash)
        {
            return;
        }
        match result {
            Ok((commit, files, parent_hash, parent_loaded)) => {
                history.detail_loading.set(false);
                history
                    .title
                    .setStringValue(&NSString::from_str(&commit.subject));
                history
                    .metadata
                    .setStringValue(&NSString::from_str(&format!(
                        "{}  ·  {}  ·  {}",
                        commit.author, commit.short_hash, commit.relative_time
                    )));
                let avatar_key = commit.author_email.as_deref().and_then(history_avatar_key);
                history.avatar_source.replace(avatar_key.clone());
                let avatar_image = avatar_key
                    .as_deref()
                    .and_then(|key| self.ivars().avatar_images.borrow().get(key).cloned())
                    .or_else(|| {
                        NSImage::imageWithSystemSymbolName_accessibilityDescription(
                            &NSString::from_str("person.crop.circle.fill"),
                            Some(&NSString::from_str(&commit.author)),
                        )
                    });
                history.avatar.setImage(avatar_image.as_deref());
                history
                    .avatar
                    .setToolTip(Some(&NSString::from_str(&commit.author)));
                history.avatar.setHidden(false);
                if let Some(email) = commit.author_email.as_deref() {
                    self.request_history_avatar(email);
                }
                history
                    .added
                    .setStringValue(&NSString::from_str(&format!("+{}", commit.insertions)));
                history
                    .deleted
                    .setStringValue(&NSString::from_str(&format!("−{}", commit.deletions)));
                history.added.setHidden(false);
                history.deleted.setHidden(false);
                history
                    .comment
                    .setStringValue(&NSString::from_str(&commit.comment));
                history.selected_commit.replace(Some(commit.clone()));
                history.selected_parent_hash.replace(parent_hash);
                history.parent_loaded.set(parent_loaded);
                history.copy_hash.setEnabled(true);
                history.open_remote.setEnabled(
                    self.ivars()
                        .repository_snapshot
                        .borrow()
                        .as_ref()
                        .and_then(|snapshot| snapshot.remote_url.as_ref())
                        .is_some(),
                );
                history.files.replace(files);
                let file_count = history.files.borrow().len();
                history
                    .file_count
                    .setStringValue(&NSString::from_str(&match file_count {
                        1 => "1 changed file".to_string(),
                        count => format!("{count} changed files"),
                    }));
                self.refresh_history_files();
                if history.files.borrow().is_empty() {
                    history
                        .empty
                        .setStringValue(&NSString::from_str("No changed files"));
                    history.empty.setHidden(false);
                } else {
                    let first = NSIndexSet::indexSetWithIndex(0);
                    history
                        .files_table
                        .selectRowIndexes_byExtendingSelection(&first, false);
                }
            }
            Err(error) => {
                history.detail_loading.set(false);
                history.pending_checkout_parent.set(false);
                history.pending_amend.set(false);
                history.parent_loaded.set(false);
                history.avatar.setHidden(true);
                history.avatar_source.borrow_mut().take();
                history.added.setHidden(true);
                history.deleted.setHidden(true);
                history
                    .metadata
                    .setStringValue(&NSString::from_str("Unable to load commit details"));
                history
                    .file_count
                    .setStringValue(&NSString::from_str("Changed files unavailable"));
                history
                    .empty
                    .setStringValue(&NSString::from_str("Unable to load commit"));
                history.empty.setHidden(false);
                self.rebuild_history_menu();
                log::warn!("native history commit failed hash={hash}: {error}");
            }
        }
    }

    fn refresh_history_files(&self) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        history.files_table.reloadData();
        if history.pending_checkout_parent.replace(false) {
            if let Some(parent_hash) = history.selected_parent_hash.borrow().clone() {
                self.run_history_action(HistoryAction::Checkout {
                    hash: parent_hash,
                    parent: true,
                });
            } else if history.parent_loaded.get() {
                self.present_path_action_error("Checkout Failed", "This commit has no parent.");
            } else {
                self.present_path_action_error(
                    "Checkout Failed",
                    "The parent commit could not be loaded. Try again.",
                );
            }
        }
        if history.pending_amend.replace(false) {
            self.show_history_amend_sheet();
        }
        self.rebuild_history_menu();
        let viewport = history.files_scroll.contentSize();
        let row_pitch =
            history.files_table.rowHeight() + history.files_table.intercellSpacing().height;
        let document_height = (history.files.borrow().len() as f64 * row_pitch)
            .max(viewport.height)
            .max(1.0);
        history
            .files_table
            .setFrameSize(NSSize::new(viewport.width.max(1.0), document_height));
        log::debug!(
            "history file pane geometry rows={} viewport_height={} document_height={}",
            history.files.borrow().len(),
            viewport.height,
            document_height
        );
        if let Some(selected) = history.selected_file.borrow().as_deref()
            && let Some(index) = history
                .files
                .borrow()
                .iter()
                .position(|file| file.path == selected)
        {
            let selection = NSIndexSet::indexSetWithIndex(index);
            history
                .files_table
                .selectRowIndexes_byExtendingSelection(&selection, false);
        }
    }

    fn request_history_comparison(&self, index: usize) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        let Some(file) = history.files.borrow().get(index).cloned() else {
            return;
        };
        let Some(hash) = history.selected_hash.borrow().clone() else {
            return;
        };
        history.selected_file.replace(Some(file.path.clone()));
        history.loaded_diff_path.borrow_mut().take();
        history.loaded_binary_path.borrow_mut().take();
        history
            .comparison_request_id
            .set(history.comparison_request_id.get().wrapping_add(1));
        let request_id = history.comparison_request_id.get();
        history.diff.setHidden(true);
        history.binary_preview.setHidden(true);
        let cached = {
            let mut cache = history.preview_cache.borrow_mut();
            cache
                .iter()
                .position(|entry| entry.hash == hash && entry.path == file.path)
                .and_then(|index| cache.remove(index))
                .map(|entry| {
                    let content = entry.content.clone();
                    cache.push_back(entry);
                    content
                })
        };
        if let Some(cached) = cached {
            log::debug!(
                "native history preview cache hit hash={hash} path={}",
                file.path
            );
            match cached {
                CachedHistoryPreviewContent::Diff(prepared) => self.apply_history_comparison(
                    self.ivars()
                        .active_workspace_id
                        .borrow()
                        .as_deref()
                        .unwrap_or_default(),
                    &hash,
                    &file.path,
                    request_id,
                    Ok(prepared),
                ),
                CachedHistoryPreviewContent::Binary(comparison) => self
                    .apply_history_bytes_comparison(
                        self.ivars()
                            .active_workspace_id
                            .borrow()
                            .as_deref()
                            .unwrap_or_default(),
                        &hash,
                        &file.path,
                        request_id,
                        Ok(comparison),
                    ),
                CachedHistoryPreviewContent::Unavailable(message) => {
                    self.show_history_comparison_error(&message)
                }
            }
            return;
        }
        history
            .empty
            .setStringValue(&NSString::from_str("Loading file diff…"));
        history.empty.setHidden(false);
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
        let request = if is_changed_binary_preview_path(&file.path) {
            RepositoryRequest::LoadHistoryBytesComparison {
                workspace_id,
                handle,
                hash,
                path: file.path,
                request_id,
                cancellation,
            }
        } else {
            RepositoryRequest::LoadHistoryComparison {
                workspace_id,
                handle,
                hash,
                path: file.path,
                request_id,
                cancellation,
            }
        };
        if let Err(error) = requests.try_send(request) {
            history
                .empty
                .setStringValue(&NSString::from_str("Unable to queue file diff"));
            log::warn!("history comparison queue rejected request error={error}");
        }
    }

    fn apply_history_comparison(
        &self,
        workspace_id: &str,
        hash: &str,
        path: &str,
        request_id: u64,
        result: Result<PreparedDiff, String>,
    ) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || history.comparison_request_id.get() != request_id
            || history.selected_hash.borrow().as_deref() != Some(hash)
            || history.selected_file.borrow().as_deref() != Some(path)
        {
            return;
        }
        match result {
            Ok(prepared) => {
                self.cache_history_preview(
                    hash,
                    path,
                    CachedHistoryPreviewContent::Diff(prepared.clone()),
                );
                self.clear_history_binary_preview();
                history.loaded_binary_path.borrow_mut().take();
                history.loaded_diff_path.replace(Some(path.to_string()));
                history.diff.set_document(
                    path,
                    prepared.fingerprint,
                    prepared.document,
                    prepared.syntax,
                );
                history.diff.setHidden(!self.is_active_page("history"));
                history.empty.setHidden(true);
            }
            Err(error) => {
                history.loaded_diff_path.borrow_mut().take();
                if is_preview_limit_message(&error) {
                    self.cache_history_preview(
                        hash,
                        path,
                        CachedHistoryPreviewContent::Unavailable(error.clone()),
                    );
                }
                self.show_history_comparison_error(&error);
                log::warn!("native history comparison failed hash={hash} path={path}: {error}");
            }
        }
    }

    fn apply_history_bytes_comparison(
        &self,
        workspace_id: &str,
        hash: &str,
        path: &str,
        request_id: u64,
        result: Result<BytesComparison, String>,
    ) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || history.comparison_request_id.get() != request_id
            || history.selected_hash.borrow().as_deref() != Some(hash)
            || history.selected_file.borrow().as_deref() != Some(path)
        {
            return;
        }
        history.diff.setHidden(true);
        match result {
            Ok(comparison) => match self.populate_native_binary_comparison(
                &history.binary_preview,
                path,
                &comparison,
            ) {
                Ok(registrations) => {
                    self.cache_history_preview(
                        hash,
                        path,
                        CachedHistoryPreviewContent::Binary(comparison),
                    );
                    history.loaded_diff_path.borrow_mut().take();
                    history.loaded_binary_path.replace(Some(path.to_string()));
                    history.binary_font_registrations.replace(registrations);
                    history
                        .binary_preview
                        .setHidden(!self.is_active_page("history"));
                    history.empty.setHidden(true);
                    log::info!("native history binary preview applied hash={hash} path={path}");
                }
                Err(error) => {
                    history.loaded_binary_path.borrow_mut().take();
                    history.binary_font_registrations.borrow_mut().clear();
                    self.show_history_comparison_error(&error);
                    log::warn!(
                        "native history binary preview failed hash={hash} path={path}: {error}"
                    );
                }
            },
            Err(error) => {
                if is_preview_limit_message(&error) {
                    self.cache_history_preview(
                        hash,
                        path,
                        CachedHistoryPreviewContent::Unavailable(error.clone()),
                    );
                }
                history.loaded_diff_path.borrow_mut().take();
                history.loaded_binary_path.borrow_mut().take();
                history.binary_font_registrations.borrow_mut().clear();
                self.show_history_comparison_error(&error);
                log::warn!(
                    "native history binary comparison failed hash={hash} path={path}: {error}"
                );
            }
        }
    }

    fn cache_history_preview(&self, hash: &str, path: &str, content: CachedHistoryPreviewContent) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        let mut cache = history.preview_cache.borrow_mut();
        if let Some(index) = cache
            .iter()
            .position(|entry| entry.hash == hash && entry.path == path)
        {
            cache.remove(index);
        }
        cache.push_back(CachedHistoryPreview {
            hash: hash.to_string(),
            path: path.to_string(),
            content,
        });
        while cache.len() > HISTORY_PREVIEW_CACHE_CAPACITY {
            cache.pop_front();
        }
    }

    fn show_history_comparison_error(&self, message: &str) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        history.diff.setHidden(true);
        self.clear_history_binary_preview();
        history.empty.setStringValue(&NSString::from_str(message));
        history.empty.setHidden(false);
    }

}
