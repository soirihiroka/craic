impl AppDelegate {
    fn show_branch_picker(&self, sender: &NSButton) {
        let popover = self.branch_picker();
        if popover.isShown() {
            popover.close();
            return;
        }
        self.ivars().branch_merge_mode.set(false);
        if let Some(search) = self.ivars().branch_search.get() {
            search.setStringValue(&NSString::new());
            search.setPlaceholderString(Some(&NSString::from_str("Search branches")));
        }
        self.update_branch_footer();
        self.refresh_branch_results("");
        popover.showRelativeToRect_ofView_preferredEdge(sender.bounds(), sender, NSRectEdge::MinY);
        if let Some(search) = self.ivars().branch_search.get()
            && let Some(window) = search.window()
        {
            window.makeFirstResponder(Some(search));
        }
    }

    fn branch_picker(&self) -> Retained<NSPopover> {
        if let Some(popover) = self.ivars().branch_popover.get() {
            return popover.clone();
        }

        let mtm = self.mtm();
        let root = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(BRANCH_PICKER_WIDTH, BRANCH_PICKER_HEIGHT),
            ),
        );
        let search = NSSearchField::new(mtm);
        search.setFrame(NSRect::new(
            NSPoint::new(12.0, BRANCH_PICKER_HEIGHT - 44.0),
            NSSize::new(BRANCH_PICKER_WIDTH - 62.0, 32.0),
        ));
        search.setPlaceholderString(Some(&NSString::from_str("Search branches")));
        search.setSendsSearchStringImmediately(true);
        unsafe {
            search.setTarget(Some(self));
            search.setAction(Some(sel!(filterBranches:)));
        }
        root.addSubview(&search);

        let add = unsafe {
            NSButton::buttonWithImage_target_action(
                &NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("plus"),
                    Some(&NSString::from_str("New branch")),
                )
                .expect("macOS 14 provides the plus SF Symbol"),
                Some(self),
                Some(sel!(addBranch:)),
                mtm,
            )
        };
        add.setFrame(NSRect::new(
            NSPoint::new(BRANCH_PICKER_WIDTH - 44.0, BRANCH_PICKER_HEIGHT - 44.0),
            NSSize::new(32.0, 32.0),
        ));
        add.setBezelStyle(NSBezelStyle::Circular);
        add.setControlSize(NSControlSize::Regular);
        add.setToolTip(Some(&NSString::from_str("New branch")));
        root.addSubview(&add);

        let list = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(BRANCH_PICKER_WIDTH - 24.0, BRANCH_PICKER_HEIGHT - 108.0),
            ),
        );
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(12.0, 56.0),
                NSSize::new(BRANCH_PICKER_WIDTH - 24.0, BRANCH_PICKER_HEIGHT - 108.0),
            ),
        );
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&list));
        root.addSubview(&scroll);

        let footer_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("arrow.triangle.merge"),
            Some(&NSString::from_str("Merge branch")),
        )
        .or_else(|| {
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("arrow.triangle.branch"),
                Some(&NSString::from_str("Merge branch")),
            )
        })
        .expect("macOS provides a branch SF Symbol");
        let footer = unsafe {
            NSButton::buttonWithTitle_image_target_action(
                &NSString::from_str("Choose a branch to merge"),
                &footer_image,
                Some(self),
                Some(sel!(toggleMergeBranch:)),
                mtm,
            )
        };
        footer.setFrame(NSRect::new(
            NSPoint::new(12.0, 12.0),
            NSSize::new(BRANCH_PICKER_WIDTH - 24.0, 32.0),
        ));
        footer.setBezelStyle(NSBezelStyle::AccessoryBar);
        footer.setControlSize(NSControlSize::Regular);
        footer.setImagePosition(NSCellImagePosition::ImageLeading);
        footer.setImageHugsTitle(true);
        root.addSubview(&footer);

        let controller = NSViewController::new(mtm);
        controller.setView(&root);
        controller.setPreferredContentSize(NSSize::new(BRANCH_PICKER_WIDTH, BRANCH_PICKER_HEIGHT));
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(NSSize::new(BRANCH_PICKER_WIDTH, BRANCH_PICKER_HEIGHT));
        popover.setContentViewController(Some(&controller));

        self.ivars()
            .branch_search
            .set(search)
            .expect("branch search is initialized once");
        self.ivars()
            .branch_list
            .set(list)
            .expect("branch result list is initialized once");
        self.ivars()
            .branch_footer
            .set(footer)
            .expect("branch footer is initialized once");
        self.ivars()
            .branch_popover
            .set(popover.clone())
            .expect("branch popover is initialized once");
        popover
    }

    fn update_branch_footer(&self) {
        let Some(footer) = self.ivars().branch_footer.get() else {
            return;
        };
        if self.ivars().branch_merge_mode.get() {
            footer.setTitle(&NSString::from_str("Cancel merge"));
            footer.setToolTip(Some(&NSString::from_str("Return to branch checkout")));
        } else {
            let branch = self
                .ivars()
                .repository_snapshot
                .borrow()
                .as_ref()
                .map(|snapshot| snapshot.branch.clone())
                .unwrap_or_else(|| "current branch".to_string());
            footer.setTitle(&NSString::from_str(&format!(
                "Choose a branch to merge into {branch}"
            )));
            footer.setToolTip(Some(&NSString::from_str(&format!(
                "Merge another branch into {branch}"
            ))));
        }
    }

    fn refresh_branch_results(&self, filter: &str) {
        let Some(list) = self.ivars().branch_list.get() else {
            return;
        };
        let subviews = list.subviews();
        for index in 0..subviews.count() {
            subviews.objectAtIndex(index).removeFromSuperview();
        }
        let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() else {
            return;
        };
        let filter = filter.trim().to_lowercase();
        let merge_mode = self.ivars().branch_merge_mode.get();
        let matches = |branch: &&craic_vcs::git::BranchInfo| {
            (!merge_mode || !branch.is_current)
                && (filter.is_empty() || branch.name.to_lowercase().contains(&filter))
                && !branch.name.starts_with("github-desktop-")
        };
        let default = snapshot
            .branches
            .iter()
            .enumerate()
            .filter(|(_, branch)| branch.is_default)
            .filter(|(_, branch)| matches(branch))
            .collect::<Vec<_>>();
        let mut recent = snapshot
            .branches
            .iter()
            .enumerate()
            .filter(|(_, branch)| !branch.is_default && branch.recent_order.is_some())
            .filter(|(_, branch)| matches(branch))
            .collect::<Vec<_>>();
        recent.sort_by_key(|(_, branch)| branch.recent_order);
        recent.truncate(5);
        let recent_names = recent
            .iter()
            .map(|(_, branch)| branch.name.as_str())
            .collect::<std::collections::HashSet<_>>();
        let other = snapshot
            .branches
            .iter()
            .enumerate()
            .filter(|(_, branch)| {
                !branch.is_default
                    && !recent_names.contains(branch.name.as_str())
                    && branch.recent_order.is_none()
            })
            .filter(|(_, branch)| matches(branch))
            .collect::<Vec<_>>();
        let groups = [
            ("Default branch", default),
            ("Recent branches", recent),
            ("Other branches", other),
        ];
        let result_count = groups
            .iter()
            .map(|(_, branches)| branches.len())
            .sum::<usize>();
        let header_count = groups
            .iter()
            .filter(|(_, branches)| !branches.is_empty())
            .count();
        let viewport_height = BRANCH_PICKER_HEIGHT - 108.0;
        let content_height = (result_count as f64 * BRANCH_ROW_HEIGHT + header_count as f64 * 24.0)
            .max(viewport_height);
        list.setFrameSize(NSSize::new(BRANCH_PICKER_WIDTH - 24.0, content_height));
        if result_count == 0 {
            let message = NSTextField::labelWithString(
                &NSString::from_str(if merge_mode {
                    "No branches are available to merge."
                } else {
                    "No matching branches."
                }),
                self.mtm(),
            );
            message.setFrame(NSRect::new(
                NSPoint::new(0.0, content_height - 48.0),
                NSSize::new(BRANCH_PICKER_WIDTH - 24.0, 28.0),
            ));
            message.setAlignment(NSTextAlignment::Center);
            message.setTextColor(Some(&NSColor::secondaryLabelColor()));
            list.addSubview(&message);
            return;
        }

        let mut y = content_height;
        for (group_name, branches) in groups {
            if branches.is_empty() {
                continue;
            }
            y -= 24.0;
            let heading = NSTextField::labelWithString(&NSString::from_str(group_name), self.mtm());
            heading.setFrame(NSRect::new(
                NSPoint::new(8.0, y),
                NSSize::new(BRANCH_PICKER_WIDTH - 40.0, 20.0),
            ));
            heading.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
            heading.setTextColor(Some(&NSColor::secondaryLabelColor()));
            list.addSubview(&heading);
            for (index, branch) in branches {
                y -= BRANCH_ROW_HEIGHT;
                let symbol = if branch.is_current {
                    "checkmark.circle.fill"
                } else {
                    "arrow.triangle.branch"
                };
                let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str("Branch")),
                )
                .expect("macOS provides branch picker SF Symbols");
                let row = unsafe {
                    NSButton::buttonWithTitle_image_target_action(
                        &NSString::from_str(&branch.name),
                        &image,
                        Some(self),
                        Some(sel!(activateBranchRow:)),
                        self.mtm(),
                    )
                };
                row.setTag(index as isize);
                row.setFrame(NSRect::new(
                    NSPoint::new(0.0, y),
                    NSSize::new(BRANCH_PICKER_WIDTH - 24.0, BRANCH_ROW_HEIGHT),
                ));
                row.setBezelStyle(NSBezelStyle::AccessoryBar);
                row.setAlignment(NSTextAlignment::Left);
                row.setImagePosition(NSCellImagePosition::ImageLeading);
                row.setImageHugsTitle(true);
                let font = if branch.is_current {
                    NSFont::boldSystemFontOfSize(13.0)
                } else {
                    NSFont::systemFontOfSize(13.0)
                };
                row.setFont(Some(&font));
                list.addSubview(&row);
            }
        }
    }

    fn show_new_branch_sheet(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("New Branch"));
        alert.setInformativeText(&NSString::from_str(
            "Create and check out a branch from the current revision.",
        ));
        alert.addButtonWithTitle(&NSString::from_str("Create"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let field = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, 24.0)),
        );
        field.setPlaceholderString(Some(&NSString::from_str("Branch name")));
        alert.setAccessoryView(Some(&field));
        let delegate = self.retain();
        let field_for_completion = field.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let branch = field_for_completion.stringValue().to_string();
            let branch = branch.trim();
            if branch.is_empty() {
                return;
            }
            let Some(requests) = delegate.ivars().repository_requests.get() else {
                return;
            };
            if let Err(error) = requests.try_send(RepositoryRequest::RunBranchAction {
                workspace_id: workspace_id.clone(),
                handle: handle.clone(),
                action: BranchAction::Create(branch.to_string()),
                cancellation: cancellation.clone(),
            }) {
                log::warn!("create branch queue rejected request error={error}");
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        alert.window().makeFirstResponder(Some(&field));
    }

}
