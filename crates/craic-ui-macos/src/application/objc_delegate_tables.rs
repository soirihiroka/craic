impl AppDelegate {
    fn handle_objc_delegate_table_view_write_rows_with_indexes_to_pasteboard(
        &self,
        table: &NSTableView,
        rows: &NSIndexSet,
        pasteboard: &NSPasteboard,
    ) -> objc2::runtime::Bool {
        let Some(files) = self.ivars().files.get() else {
            return false.into();
        };
        if !std::ptr::eq(table, &**files.table) || files.mutation_in_progress.get() {
            return false.into();
        }
        let Some(row) = self.filtered_file_tree_rows().get(rows.firstIndex()).cloned() else {
            return false.into();
        };
        if row.info.path.is_root() || !row.info.capabilities.readable {
            return false.into();
        }
        let Some(access) = self
            .ivars()
            .workspace_handle
            .borrow()
            .as_ref()
            .map(|handle| handle.workspace_files())
        else {
            return false.into();
        };
        let Some(workspace) = self
            .ivars()
            .active_workspace_id
            .borrow()
            .as_deref()
            .and_then(|active| {
                self.ivars()
                    .workspaces
                    .borrow()
                    .iter()
                    .find(|workspace| workspace.selection_id() == active)
                    .map(|workspace| workspace.workspace.clone())
            })
        else {
            return false.into();
        };
        let drag_type = workspace_file_drag_type();
        pasteboard.clearContents();
        let Some(payload) = workspace_file_drag_payload(&workspace, access.as_ref(), &row.info.path)
        else {
            return false.into();
        };
        if !pasteboard.setString_forType(&NSString::from_str(&payload), &drag_type) {
            return false.into();
        }
        if let Some(local_path) = access.local_path(&row.info.path) {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&local_path.to_string_lossy()));
            if let Some(value) = url.absoluteString() {
                let file_url_type = unsafe { NSPasteboardTypeFileURL };
                pasteboard.setString_forType(&value, file_url_type);
            }
        }
        true.into()
    }

    fn handle_objc_delegate_table_view_validate_drop_proposed_row_proposed_drop_operation(
        &self,
        table: &NSTableView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
        row: isize,
        _drop_operation: NSTableViewDropOperation,
    ) -> NSDragOperation {
        let Some(files) = self.ivars().files.get() else {
            return NSDragOperation::None;
        };
        if !std::ptr::eq(table, &**files.table) || files.mutation_in_progress.get() {
            self.clear_file_drop_hover();
            return NSDragOperation::None;
        }
        let Some((access, destination_parent)) = self.workspace_file_drop_parent(row) else {
            self.clear_file_drop_hover();
            return NSDragOperation::None;
        };
        self.schedule_file_drop_auto_expand(&destination_parent);
        let pasteboard = info.draggingPasteboard();
        let drag_type = workspace_file_drag_type();
        let internal_types = NSArray::from_slice(&[&*drag_type]);
        if pasteboard.availableTypeFromArray(&internal_types).is_some()
            && let Some(drag) = workspace_file_drag_source(info)
        {
            if !self
                .ivars()
                .workspaces
                .borrow()
                .iter()
                .any(|workspace| {
                    workspace.selection_id() == drag.workspace_selection_id
                })
            {
                return NSDragOperation::None;
            }
            if drag.workspace_id != access.workspace().id.as_str() {
                table.setDropRow_dropOperation(row, NSTableViewDropOperation::On);
                return NSDragOperation::Copy;
            }
            let source = access.root().join_child(&drag.relative);
            let Some(source_info) = files
                .rows
                .borrow()
                .iter()
                .find(|row| row.info.path == source)
                .map(|row| row.info.clone())
            else {
                return NSDragOperation::None;
            };
            if destination_parent == source
                || source.parent().as_ref() == Some(&destination_parent)
                || destination_parent.is_descendant_of(&source)
            {
                return NSDragOperation::None;
            }
            let copy = current_drag_requests_copy() || !source_info.capabilities.movable;
            let operation = if copy && source_info.capabilities.readable {
                NSDragOperation::Copy
            } else if source_info.capabilities.movable {
                NSDragOperation::Move
            } else {
                NSDragOperation::None
            };
            if operation != NSDragOperation::None {
                table.setDropRow_dropOperation(row, NSTableViewDropOperation::On);
            }
            return operation;
        }
        let file_url_type = unsafe { NSPasteboardTypeFileURL };
        let types = NSArray::from_slice(&[file_url_type]);
        if pasteboard.availableTypeFromArray(&types).is_none() {
            return NSDragOperation::None;
        }
        table.setDropRow_dropOperation(row, NSTableViewDropOperation::On);
        NSDragOperation::Copy
    }

    fn handle_objc_delegate_table_view_accept_drop_row_drop_operation(
        &self,
        table: &NSTableView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
        row: isize,
        _drop_operation: NSTableViewDropOperation,
    ) -> objc2::runtime::Bool {
        let Some(files) = self.ivars().files.get() else {
            return false.into();
        };
        if !std::ptr::eq(table, &**files.table) || files.mutation_in_progress.get() {
            self.clear_file_drop_hover();
            return false.into();
        }
        let Some((access, destination_parent)) = self.workspace_file_drop_parent(row) else {
            self.clear_file_drop_hover();
            return false.into();
        };
        self.clear_file_drop_hover();
        if let Some(drag) = workspace_file_drag_source(info) {
            if drag.workspace_id != access.workspace().id.as_str() {
                let Some(source_workspace) = self
                    .ivars()
                    .workspaces
                    .borrow()
                    .iter()
                    .find(|workspace| {
                        workspace.selection_id() == drag.workspace_selection_id
                    })
                    .map(|workspace| workspace.workspace.clone())
                else {
                    return false.into();
                };
                let Some(name) = drag
                    .relative
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                else {
                    return false.into();
                };
                self.request_file_mutation(
                    access,
                    NativeFileMutation::Transfer {
                        source_workspace,
                        source_workspace_id: drag.workspace_id,
                        source_relative: drag.relative,
                        destination: destination_parent.join_child(name),
                    },
                );
                return true.into();
            }
            let source = access.root().join_child(&drag.relative);
            let Some(source_info) = files
                .rows
                .borrow()
                .iter()
                .find(|row| row.info.path == source)
                .map(|row| row.info.clone())
            else {
                return false.into();
            };
            if destination_parent == source
                || source.parent().as_ref() == Some(&destination_parent)
                || destination_parent.is_descendant_of(&source)
            {
                return false.into();
            }
            let Some(name) = source.file_name().map(ToString::to_string) else {
                return false.into();
            };
            let copy = current_drag_requests_copy() || !source_info.capabilities.movable;
            if copy {
                self.request_file_mutation(
                    access,
                    NativeFileMutation::Copy {
                        source,
                        destination: destination_parent.join_child(name),
                    },
                );
            } else {
                self.request_file_mutation(
                    access,
                    NativeFileMutation::Move {
                        source,
                        destination_parent,
                        new_name: name,
                    },
                );
            }
            return true.into();
        }
        let sources = local_file_paths_from_drag(info);
        if sources.is_empty() {
            return false.into();
        }
        self.request_file_mutation(
            access,
            NativeFileMutation::Upload {
                sources,
                destination_parent,
            },
        );
        true.into()
    }

    fn handle_objc_delegate_table_view_is_group_row(&self, table: &NSTableView, row: isize) -> bool {
        self.ivars()
            .containers
            .get()
            .is_some_and(|containers| std::ptr::eq(table, &**containers.table))
            && usize::try_from(row).ok().is_some_and(|row| {
                matches!(
                    self.filtered_container_rows().get(row),
                    Some(NativeContainerRow::Group(_))
                )
            })
    }

    fn handle_objc_delegate_table_view_view_for_table_column_row(
        &self,
        table: &NSTableView,
        column: Option<&NSTableColumn>,
        row: isize,
    ) -> Option<Retained<NSView>> {
        if self
            .ivars()
            .workspace_table
            .get()
            .is_some_and(|workspace_table| std::ptr::eq(table, &**workspace_table))
        {
            usize::try_from(row)
                .ok()
                .and_then(|row| self.make_workspace_cell(table, row))
        } else if self
            .ivars()
            .author_table
            .get()
            .is_some_and(|author_table| std::ptr::eq(table, &**author_table))
        {
            usize::try_from(row)
                .ok()
                .and_then(|row| self.make_commit_author_cell(table, row))
        } else if self
            .ivars()
            .agents
            .get()
            .is_some_and(|agents| std::ptr::eq(table, &*agents.transcript_table))
        {
            usize::try_from(row)
                .ok()
                .and_then(|row| self.make_native_agent_transcript_cell(table, row))
        } else if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &*files.preview_table))
        {
            usize::try_from(row)
                .ok()
                .and_then(|row| self.make_csv_table_cell(table, column?, row))
        } else if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &**files.table))
        {
            usize::try_from(row)
                .ok()
                .and_then(|row| self.make_file_tree_cell(table, row))
        } else if self
            .ivars()
            .containers
            .get()
            .is_some_and(|containers| std::ptr::eq(table, &**containers.table))
        {
            usize::try_from(row)
                .ok()
                .and_then(|row| self.make_container_cell(table, column, row))
        } else if let Some(history) = self.ivars().history.get() {
            let row = usize::try_from(row).ok();
            if std::ptr::eq(table, &*history.files_table) {
                row.and_then(|row| history.files.borrow().get(row).cloned())
                    .map(|file| {
                        let width = table.bounds().size.width.max(180.0);
                        let cell = NSView::initWithFrame(
                            NSView::alloc(self.mtm()),
                            NSRect::new(
                                NSPoint::new(0.0, 0.0),
                                NSSize::new(width, 32.0),
                            ),
                        );
                        let title = NSTextField::labelWithString(
                            &NSString::from_str(&file.path),
                            self.mtm(),
                        );
                        title.setFrame(NSRect::new(
                            NSPoint::new(10.0, 6.0),
                            NSSize::new((width - 50.0).max(1.0), 20.0),
                        ));
                        title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                        title.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                        title.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
                        title.setToolTip(Some(&NSString::from_str(&file.path)));
                        cell.addSubview(&title);

                        let (symbol, description) = changed_file_symbol(&file.status);
                        if let Some(image) =
                            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                                &NSString::from_str(symbol),
                                Some(&NSString::from_str(description)),
                            )
                        {
                            let status = NSImageView::imageViewWithImage(&image, self.mtm());
                            status.setFrame(NSRect::new(
                                NSPoint::new(width - 34.0, 8.0),
                                NSSize::new(16.0, 16.0),
                            ));
                            status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
                            status.setToolTip(Some(&NSString::from_str(description)));
                            cell.addSubview(&status);
                        }
                        cell
                    })
            } else if std::ptr::eq(table, &*history.table) {
                if let Some(row) = row {
                    let commit = history.commits.borrow().get(row).cloned();
                    if let Some(commit) = commit {
            let width = table.bounds().size.width.max(180.0);
            let cell = NSView::initWithFrame(
                NSView::alloc(self.mtm()),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, 54.0)),
            );

            let added = NSTextField::labelWithString(
                &NSString::from_str(&format!("+{}", commit.insertions)),
                self.mtm(),
            );
            added.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            added.setTextColor(Some(&NSColor::systemGreenColor()));
            added.sizeToFit();
            let added_size = added.frame().size;

            let deleted = NSTextField::labelWithString(
                &NSString::from_str(&format!("-{}", commit.deletions)),
                self.mtm(),
            );
            deleted.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            deleted.setTextColor(Some(&NSColor::systemRedColor()));
            deleted.sizeToFit();
            let deleted_size = deleted.frame().size;
            let deleted_x = width - 10.0 - deleted_size.width;
            let added_x = deleted_x - 5.0 - added_size.width;
            added.setFrame(NSRect::new(
                NSPoint::new(added_x, 31.0),
                NSSize::new(added_size.width, 17.0),
            ));
            deleted.setFrame(NSRect::new(
                NSPoint::new(deleted_x, 31.0),
                NSSize::new(deleted_size.width, 17.0),
            ));
            added.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            deleted.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            cell.addSubview(&added);
            cell.addSubview(&deleted);

            let mut subject_trailing = added_x - 8.0;
            if !commit.tags.is_empty() {
                let tags_width = commit
                    .tags
                    .iter()
                    .map(|tag| tag.chars().count() as f64 * 6.5 + 22.0)
                    .sum::<f64>()
                    .min(130.0)
                    .max(48.0);
                let tags = NSTokenField::initWithFrame(
                    NSTokenField::alloc(self.mtm()),
                    NSRect::new(
                        NSPoint::new((subject_trailing - tags_width).max(60.0), 27.0),
                        NSSize::new(tags_width, 22.0),
                    ),
                );
                tags.setEditable(false);
                tags.setSelectable(false);
                tags.setBordered(false);
                tags.setDrawsBackground(false);
                tags.setTokenStyle(NSTokenStyle::Rounded);
                tags.setFont(Some(&NSFont::systemFontOfSize(10.0)));
                tags.setStringValue(&NSString::from_str(&commit.tags.join(",")));
                tags.setToolTip(Some(&NSString::from_str(&commit.tags.join(", "))));
                tags.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
                subject_trailing = tags.frame().origin.x - 8.0;
                cell.addSubview(&tags);
            }

            let subject = NSTextField::labelWithString(
                &NSString::from_str(if commit.subject.is_empty() {
                    "Untitled commit"
                } else {
                    &commit.subject
                }),
                self.mtm(),
            );
            subject.setFrame(NSRect::new(
                NSPoint::new(10.0, 28.0),
                NSSize::new((subject_trailing - 10.0).max(1.0), 20.0),
            ));
            subject.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            subject.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            subject.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            subject.setToolTip(Some(&NSString::from_str(&commit.subject)));
            cell.addSubview(&subject);

            let avatar_key = commit
                .author_email
                .as_deref()
                .and_then(history_avatar_key);
            let avatar_image = avatar_key
                .as_deref()
                .and_then(|key| self.ivars().avatar_images.borrow().get(key).cloned())
                .or_else(|| {
                    NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str("person.crop.circle.fill"),
                        Some(&NSString::from_str(&commit.author)),
                    )
                });
            if let Some(image) = avatar_image {
                let avatar = NSImageView::imageViewWithImage(&image, self.mtm());
                avatar.setFrame(NSRect::new(
                    NSPoint::new(11.5, 8.0),
                    NSSize::new(18.0, 18.0),
                ));
                avatar.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                avatar.setWantsLayer(true);
                if let Some(layer) = avatar.layer() {
                    layer.setCornerRadius(9.0);
                    layer.setMasksToBounds(true);
                }
                avatar.setToolTip(Some(&NSString::from_str(&commit.author)));
                cell.addSubview(&avatar);
            }
            if let Some(email) = commit.author_email.as_deref() {
                self.request_history_avatar(email);
            }

            let author_hash = NSTextField::labelWithString(
                &NSString::from_str(&format!(
                    "{}  ·  {}",
                    commit.author, commit.short_hash
                )),
                self.mtm(),
            );
            author_hash.setFrame(NSRect::new(
                NSPoint::new(34.0, 5.0),
                NSSize::new((width - 34.0 - 104.0).max(1.0), 18.0),
            ));
            author_hash.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            author_hash.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            author_hash.setTextColor(Some(&NSColor::secondaryLabelColor()));
            author_hash.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            cell.addSubview(&author_hash);

            let time = NSTextField::labelWithString(
                &NSString::from_str(&commit.relative_time),
                self.mtm(),
            );
            time.setFrame(NSRect::new(
                NSPoint::new(width - 100.0, 5.0),
                NSSize::new(90.0, 18.0),
            ));
            time.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            time.setAlignment(NSTextAlignment::Right);
            time.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            time.setTextColor(Some(&NSColor::secondaryLabelColor()));
            time.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
            cell.addSubview(&time);
            Some(cell)
                    } else if history.loading.get() && row == history.commits.borrow().len() {
                    let width = table.bounds().size.width.max(180.0);
                    let cell = NSView::initWithFrame(
                        NSView::alloc(self.mtm()),
                        NSRect::new(
                            NSPoint::new(0.0, 0.0),
                            NSSize::new(width, HISTORY_LOADING_ROW_HEIGHT),
                        ),
                    );
                    cell.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                    let message = if history.query.borrow().is_empty() {
                        "Loading more commits..."
                    } else {
                        "Searching more commits..."
                    };
                    let label = NSTextField::labelWithString(
                        &NSString::from_str(message),
                        self.mtm(),
                    );
                    label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                    label.sizeToFit();
                    let label_size = label.frame().size;
                    let row_height = HISTORY_LOADING_ROW_HEIGHT;
                    let spinner_size = 16.0;
                    let gap = 12.0;
                    let group_width = spinner_size + gap + label_size.width;
                    let group_x = (width - group_width) / 2.0;
                    let spinner = NSProgressIndicator::initWithFrame(
                        NSProgressIndicator::alloc(self.mtm()),
                        NSRect::new(
                            NSPoint::new(group_x, (row_height - spinner_size) / 2.0),
                            NSSize::new(spinner_size, spinner_size),
                        ),
                    );
                    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                    spinner.setControlSize(NSControlSize::Small);
                    spinner.setIndeterminate(true);
                    spinner.setDisplayedWhenStopped(false);
                    spinner.setAutoresizingMask(
                        NSAutoresizingMaskOptions::ViewMinXMargin
                            | NSAutoresizingMaskOptions::ViewMaxXMargin,
                    );
                    // SAFETY: AppKit creates and animates this native table-row spinner on
                    // the main thread, and the table retains it with the row view.
                    unsafe { spinner.startAnimation(None) };
                    cell.addSubview(&spinner);
                    label.setFrame(NSRect::new(
                        NSPoint::new(
                            group_x + spinner_size + gap,
                            (row_height - label_size.height) / 2.0,
                        ),
                        label_size,
                    ));
                    label.setAutoresizingMask(
                        NSAutoresizingMaskOptions::ViewMinXMargin
                            | NSAutoresizingMaskOptions::ViewMaxXMargin,
                    );
                    cell.addSubview(&label);
                    Some(cell)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    fn handle_objc_delegate_table_view_row_view_for_row(
        &self,
        table: &NSTableView,
        _row: isize,
    ) -> Option<Retained<NSTableRowView>> {
        if self
            .ivars()
            .workspace_table
            .get()
            .is_some_and(|workspace_table| std::ptr::eq(table, &**workspace_table))
        {
            Some(NSTableRowView::new(self.mtm()))
        } else if self
            .ivars()
            .author_table
            .get()
            .is_some_and(|author_table| std::ptr::eq(table, &**author_table))
        {
            Some(NSTableRowView::new(self.mtm()))
        } else if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &**files.table))
        {
            Some(NSTableRowView::new(self.mtm()))
        } else if self
            .ivars()
            .containers
            .get()
            .is_some_and(|containers| std::ptr::eq(table, &**containers.table))
        {
            Some(NSTableRowView::new(self.mtm()))
        } else if let Some(history) = self.ivars().history.get()
            && std::ptr::eq(table, &*history.table)
        {
            let row_view = NSTableRowView::new(self.mtm());
            row_view.setEmphasized(false);
            Some(row_view)
        } else {
            None
        }
    }

    fn handle_objc_delegate_table_view_selection_did_change(&self, notification: &NSNotification) {
        let Some(table) = notification
            .object()
            .and_then(|object| object.downcast::<NSTableView>().ok())
        else {
            return;
        };
        if self
            .ivars()
            .workspace_table
            .get()
            .is_some_and(|workspace_table| std::ptr::eq(&*table, &**workspace_table))
        {
            return;
        }
        if self
            .ivars()
            .author_table
            .get()
            .is_some_and(|author_table| std::ptr::eq(&*table, &**author_table))
        {
            if self.ivars().author_selection_suppressed.get() {
                return;
            }
            let row = table.selectedRow();
            if let Ok(index) = usize::try_from(row)
                && index < self.ivars().author_options.borrow().len()
            {
                self.select_commit_author_at(index);
            }
            return;
        }
        if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(&*table, &**files.table))
        {
            if let Ok(row) = usize::try_from(table.selectedRow()) {
                self.select_file_tree_row(row);
            }
            return;
        }
        if self
            .ivars()
            .containers
            .get()
            .is_some_and(|containers| std::ptr::eq(&*table, &**containers.table))
        {
            if let Ok(row) = usize::try_from(table.selectedRow()) {
                if self
                    .ivars()
                    .containers
                    .get()
                    .is_some_and(|containers| containers.context_selection.replace(false))
                {
                    if let Some(row) = self.filtered_container_rows().get(row).cloned() {
                        self.display_container_row(row, false);
                    }
                } else {
                    self.select_container_row(row);
                }
            }
            return;
        }
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        let row = table.selectedRow();
        if row >= 0 && std::ptr::eq(&*table, &*history.table) {
            if let Some(row_view) = table.rowViewAtRow_makeIfNecessary(row, false) {
                row_view.setEmphasized(false);
            }
            self.select_history_commit(row as usize);
        } else if row >= 0 && std::ptr::eq(&*table, &*history.files_table) {
            self.request_history_comparison(row as usize);
        }
    }

    fn handle_objc_delegate_table_view_did_click_table_column(
        &self,
        table: &NSTableView,
        table_column: &NSTableColumn,
    ) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if !std::ptr::eq(table, &*files.preview_table)
            || files.sqlite_controls.isHidden()
        {
            return;
        }
        let Some(column_index) = table
            .tableColumns()
            .iter()
            .position(|column| std::ptr::eq(&*column, table_column))
        else {
            return;
        };
        let next_sort = {
            let mut state = files.sqlite_state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return;
            };
            state.page = 0;
            state.sort = match state.sort.as_ref() {
                Some(sort)
                    if sort.column_index == column_index
                        && sort.direction == NativeSqliteSortDirection::Ascending =>
                {
                    Some(NativeSqliteSort {
                        column_index,
                        direction: NativeSqliteSortDirection::Descending,
                    })
                }
                Some(sort)
                    if sort.column_index == column_index
                        && sort.direction == NativeSqliteSortDirection::Descending =>
                {
                    None
                }
                _ => Some(NativeSqliteSort {
                    column_index,
                    direction: NativeSqliteSortDirection::Ascending,
                }),
            };
            state.sort.clone()
        };
        log::debug!(
            "native SQLite sort changed column={} sort={next_sort:?}",
            column_index
        );
        self.request_workspace_sqlite_page();
    }

    fn handle_objc_delegate_table_view_should_select_row(
        &self,
        table: &NSTableView,
        row: isize,
    ) -> objc2::runtime::Bool {
        if self
            .ivars()
            .workspace_table
            .get()
            .is_some_and(|workspace_table| std::ptr::eq(table, &**workspace_table))
        {
            return (row >= 0
                && (row as usize) < self.ivars().workspace_results.borrow().len())
            .into();
        }
        if self
            .ivars()
            .author_table
            .get()
            .is_some_and(|author_table| std::ptr::eq(table, &**author_table))
        {
            return (row >= 0 && (row as usize) < self.ivars().author_options.borrow().len())
                .into();
        }
        if self
            .ivars()
            .agents
            .get()
            .is_some_and(|agents| std::ptr::eq(table, &*agents.transcript_table))
        {
            return false.into();
        }
        if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &*files.preview_table))
        {
            return (row >= 0).into();
        }
        if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &**files.table))
        {
            return (row >= 0
                && (row as usize) < self.filtered_file_tree_rows().len())
                .into();
        }
        if self
            .ivars()
            .containers
            .get()
            .is_some_and(|containers| std::ptr::eq(table, &**containers.table))
        {
            return (row >= 0 && (row as usize) < self.filtered_container_rows().len()).into();
        }
        let Some(history) = self.ivars().history.get() else {
            return false.into();
        };
        if std::ptr::eq(table, &*history.table) {
            return (row >= 0 && (row as usize) < history.commits.borrow().len()).into();
        }
        true.into()
    }
}
