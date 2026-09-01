impl AppDelegate {
    fn make_csv_table_cell(
        &self,
        table: &NSTableView,
        column: &NSTableColumn,
        row: usize,
    ) -> Option<Retained<NSView>> {
        let files = self.ivars().files.get()?;
        let column_index = table
            .tableColumns()
            .iter()
            .position(|candidate| std::ptr::eq(&*candidate, column))?;
        let value = files
            .preview_table_rows
            .borrow()
            .get(row)
            .and_then(|record| record.get(column_index))
            .cloned()
            .unwrap_or_default();
        let width = column.width().max(80.0);
        let cell = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, 26.0)),
        );
        let label = NSTextField::labelWithString(&NSString::from_str(&value), self.mtm());
        label.setFrame(NSRect::new(
            NSPoint::new(7.0, 4.0),
            NSSize::new((width - 14.0).max(1.0), 18.0),
        ));
        label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        label.setFont(Some(&NSFont::systemFontOfSize(11.5)));
        label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        label.setSelectable(true);
        label.setToolTip(Some(&NSString::from_str(&value)));
        cell.addSubview(&label);
        Some(cell)
    }

    fn make_file_tree_cell(&self, table: &NSTableView, row: usize) -> Option<Retained<NSView>> {
        let files = self.ivars().files.get()?;
        let row_data = self.filtered_file_tree_rows().get(row).cloned()?;
        // Keep the row tied to the actual table width. Giving a narrow table an artificial
        // minimum width lets its text field extend underneath the detail pane instead of
        // allowing AppKit's native middle truncation to take effect.
        let width = table.bounds().size.width.max(1.0);
        let cell = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, FILE_ROW_HEIGHT)),
        );
        let indent = 8.0 + row_data.depth as f64 * 15.0;
        let listable = row_data.info.capabilities.listable;
        if listable {
            let expanded = files.expanded.borrow().contains(&row_data.info.path);
            let disclosure_image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(if expanded {
                    "chevron.down"
                } else {
                    "chevron.right"
                }),
                Some(&NSString::from_str(if expanded {
                    "Collapse folder"
                } else {
                    "Expand folder"
                })),
            )?;
            let disclosure = unsafe {
                NSButton::buttonWithImage_target_action(
                    &disclosure_image,
                    Some(self),
                    Some(sel!(toggleFileDirectory:)),
                    self.mtm(),
                )
            };
            disclosure.setFrame(NSRect::new(
                NSPoint::new(indent, 3.0),
                NSSize::new(22.0, 24.0),
            ));
            disclosure.setTag(row as isize);
            disclosure.setBordered(false);
            disclosure.setToolTip(Some(&NSString::from_str(if expanded {
                "Collapse folder"
            } else {
                "Expand folder"
            })));
            cell.addSubview(&disclosure);
        }

        let icon_image = self.native_file_row_icon(&row_data.info)?;
        let icon = NSImageView::imageViewWithImage(&icon_image, self.mtm());
        let icon_x = indent + 24.0;
        icon.setFrame(NSRect::new(
            NSPoint::new(icon_x, 6.0),
            NSSize::new(17.0, 17.0),
        ));
        icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        let icon_color = if row_data.info.git_ignored == Some(true) {
            NSColor::tertiaryLabelColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        icon.setContentTintColor(Some(&icon_color));
        cell.addSubview(&icon);

        let ignored_width = if row_data.info.git_ignored == Some(true) {
            54.0
        } else {
            8.0
        };
        let title_x = icon_x + 23.0;
        let title = NSTextField::labelWithString(
            &NSString::from_str(&row_data.info.display_name),
            self.mtm(),
        );
        title.setFrame(NSRect::new(
            NSPoint::new(title_x, 6.0),
            NSSize::new((width - title_x - ignored_width).max(1.0), 18.0),
        ));
        title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        title.setMaximumNumberOfLines(1);
        if let Some(cell) = title.cell() {
            cell.setUsesSingleLineMode(true);
            cell.setTruncatesLastVisibleLine(true);
        }
        title.setFont(Some(&NSFont::systemFontOfSize(12.5)));
        let title_color = if row_data.info.git_ignored == Some(true) {
            NSColor::tertiaryLabelColor()
        } else {
            NSColor::labelColor()
        };
        title.setTextColor(Some(&title_color));
        title.setToolTip(Some(&NSString::from_str(&row_data.info.path.display())));
        cell.addSubview(&title);

        if row_data.info.git_ignored == Some(true) {
            let ignored = NSTextField::labelWithString(&NSString::from_str("Ignored"), self.mtm());
            ignored.setFrame(NSRect::new(
                NSPoint::new(width - 52.0, 7.0),
                NSSize::new(46.0, 16.0),
            ));
            ignored.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            ignored.setAlignment(NSTextAlignment::Right);
            ignored.setFont(Some(&NSFont::systemFontOfSize(9.5)));
            ignored.setTextColor(Some(&NSColor::tertiaryLabelColor()));
            cell.addSubview(&ignored);
        }
        Some(cell)
    }

    fn native_file_row_icon(&self, info: &FileNodeInfo) -> Option<Retained<NSImage>> {
        let name = info.display_name.to_ascii_lowercase();
        let path = info.path.display();
        let support = resolve_file_support(FileProbe {
            path: &path,
            is_dir: info.kind == FileNodeKind::Directory,
            leading_bytes: None,
        });
        let symbol = match info.kind {
            FileNodeKind::Directory => "folder",
            FileNodeKind::Archive { .. } => "archivebox",
            FileNodeKind::Symlink => "link",
            FileNodeKind::Other => "questionmark.square.dashed",
            FileNodeKind::File
                if matches!(
                    name.as_str(),
                    ".gitignore" | ".gitattributes" | ".gitmodules"
                ) =>
            {
                "arrow.triangle.branch"
            }
            FileNodeKind::File if name == "readme.md" => "book.closed",
            FileNodeKind::File if name == "todo.md" => "checklist",
            FileNodeKind::File
                if name == "license"
                    || name.starts_with("license.")
                    || name == "copying"
                    || name.starts_with("copying.") =>
            {
                "checkmark.seal"
            }
            FileNodeKind::File if name == ".env" || name.starts_with(".env.") => "key",
            FileNodeKind::File if support.role.is_some() => "terminal",
            FileNodeKind::File if name.ends_with(".lock") => "lock.doc",
            FileNodeKind::File => match support.content_kind {
                ContentKind::Folder => "folder",
                ContentKind::Markdown | ContentKind::Rst => "doc.text",
                ContentKind::Html => "globe",
                ContentKind::Svg | ContentKind::Image => "photo",
                ContentKind::Audio => "waveform",
                ContentKind::Video => "film",
                ContentKind::Font => "textformat",
                ContentKind::Pdf => "doc.richtext",
                ContentKind::Sqlite => "cylinder",
                ContentKind::Notebook => "tablecells",
                ContentKind::Safetensors => "cube",
                ContentKind::Text => match support.language {
                    LanguageId::Bash | LanguageId::Make => "terminal",
                    LanguageId::Ini
                    | LanguageId::Json
                    | LanguageId::Toml
                    | LanguageId::Yaml => "gearshape",
                    LanguageId::PlainText => "doc",
                    _ => "chevron.left.forwardslash.chevron.right",
                },
            },
        };
        let description = NSString::from_str(&info.display_name);
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&description),
        )
        .or_else(|| {
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("doc"),
                Some(&description),
            )
        })
    }

    fn select_file_tree_row(&self, row: usize) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(row_data) = self.filtered_file_tree_rows().get(row).cloned() else {
            return;
        };
        let loaded_text_path = files.loaded_text_path.borrow().clone();
        if loaded_text_path.as_ref() != Some(&row_data.info.path) && files.text_dirty.get() {
            files
                .pending_text_selection
                .replace(Some(row_data.info.path.clone()));
            self.flush_workspace_file_save();
            if let Some(current_path) = loaded_text_path
                && let Some(current_row) = self
                    .filtered_file_tree_rows()
                    .iter()
                    .position(|candidate| candidate.info.path == current_path)
            {
                files.table.selectRowIndexes_byExtendingSelection(
                    &NSIndexSet::indexSetWithIndex(current_row),
                    false,
                );
                files.table.scrollRowToVisible(current_row as isize);
            }
            return;
        }
        if loaded_text_path.as_ref() != Some(&row_data.info.path) {
            self.clear_font_preview();
            self.clear_sqlite_preview();
            files.loaded_text_path.borrow_mut().take();
            files.loaded_text_signature.borrow_mut().take();
            files.text_buffer.borrow_mut().clear();
            files.text_selection.set(NSRange::new(0, 0));
            files.text_editable.set(false);
            files.preview_code.clear_completions();
            files.text_dirty.set(false);
            files.preview_text.setEditable(false);
            files.preview_web_mode.set(NativeWebPreviewMode::Hidden);
            files.preview_web.setHidden(true);
            files.preview_divider.setHidden(true);
            files.preview_pdf.setHidden(true);
            // SAFETY: Selection changes are handled on AppKit's main thread.
            unsafe { files.preview_pdf.setDocument(None) };
            self.clear_csv_table_preview();
            // SAFETY: Selection changes are handled on AppKit's main thread.
            unsafe { files.preview_web.stopLoading() };
        }
        files
            .selected_path
            .replace(Some(row_data.info.path.clone()));
        files
            .title
            .setStringValue(&NSString::from_str(&row_data.info.display_name));
        let kind = match row_data.info.kind {
            FileNodeKind::Directory => "Folder",
            FileNodeKind::Archive { .. } => "Archive",
            FileNodeKind::Symlink => "Symbolic link",
            FileNodeKind::File => "File",
            FileNodeKind::Other => "Item",
        };
        let size = row_data
            .info
            .len
            .map(|bytes| format!(" · {bytes} bytes"))
            .unwrap_or_default();
        let startup_location = self
            .ivars()
            .pending_files_path
            .borrow()
            .as_deref()
            .filter(|path| *path == row_data.info.path.display())
            .and_then(|_| self.ivars().pending_files_line.get())
            .map(|line| match self.ivars().pending_files_column.get() {
                Some(column) => format!(" · line {line}, column {column}"),
                None => format!(" · line {line}"),
            })
            .unwrap_or_default();
        let metadata = format!(
            "{kind}{size} · {}{startup_location}",
            row_data.info.path.display()
        );
        files.metadata_base.replace(metadata.clone());
        files
            .metadata
            .setStringValue(&NSString::from_str(&metadata));
        let preserve_loaded_text = files.loaded_text_path.borrow().as_ref()
            == Some(&row_data.info.path)
            && (files.text_dirty.get()
                || files.loaded_text_signature.borrow().as_ref()
                    == Some(&file_signature_from_info(&row_data.info)));
        if row_data.info.capabilities.readable
            && !is_sqlite_preview_path(&row_data.info.path.display())
            && preserve_loaded_text
        {
            files.preview_code.setHidden(false);
            files.empty.setHidden(true);
            self.layout_files_preview();
            return;
        }
        files.preview_scroll.setHidden(true);
        files.preview_code.setHidden(true);
        files.preview_image.setHidden(true);
        files.preview_image.clear_image();
        unsafe { files.preview_spinner.stopAnimation(None) };
        files.preview_spinner.setHidden(true);
        if row_data.info.capabilities.listable {
            self.request_workspace_folder(row_data.info.path.clone(), row_data.info);
        } else if row_data.info.capabilities.readable {
            if is_sqlite_preview_path(&row_data.info.path.display()) {
                self.request_workspace_sqlite(row_data.info.path.clone(), row_data.info, None);
                return;
            }
            self.request_workspace_file(row_data.info.path);
        } else {
            files
                .empty
                .setStringValue(&NSString::from_str("This item cannot be previewed."));
            files.empty.setHidden(false);
        }
    }

    fn request_workspace_sqlite(
        &self,
        path: FileNodePath,
        info: FileNodeInfo,
        prefetched_bytes: Option<Vec<u8>>,
    ) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        self.clear_sqlite_preview();
        let request_id = files.preview_request_id.get().wrapping_add(1);
        files.preview_request_id.set(request_id);
        files
            .empty
            .setStringValue(&NSString::from_str("Loading SQLite schema…"));
        files.empty.setHidden(false);
        files.preview_spinner.setHidden(false);
        unsafe { files.preview_spinner.startAnimation(None) };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadWorkspaceSqliteSchema {
            workspace_id,
            handle,
            path,
            info,
            prefetched_bytes,
            request_id,
            cancellation,
        }) {
            unsafe { files.preview_spinner.stopAnimation(None) };
            files.preview_spinner.setHidden(true);
            files.empty.setStringValue(&NSString::from_str(&format!(
                "Unable to queue SQLite preview: {error}"
            )));
        }
    }

    fn request_workspace_folder(&self, path: FileNodePath, info: FileNodeInfo) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let request_id = files.preview_request_id.get().wrapping_add(1);
        files.preview_request_id.set(request_id);
        files
            .empty
            .setStringValue(&NSString::from_str("Loading folder details…"));
        files.empty.setHidden(false);
        files.preview_spinner.setHidden(false);
        unsafe { files.preview_spinner.startAnimation(None) };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadWorkspaceFolder {
            workspace_id,
            handle,
            path,
            info,
            request_id,
            cancellation,
        }) {
            unsafe { files.preview_spinner.stopAnimation(None) };
            files.preview_spinner.setHidden(true);
            files.empty.setStringValue(&NSString::from_str(&format!(
                "Unable to queue folder preview: {error}"
            )));
        }
    }

    fn request_workspace_file(&self, path: FileNodePath) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let request_id = files.preview_request_id.get().wrapping_add(1);
        files.preview_request_id.set(request_id);
        files
            .empty
            .setStringValue(&NSString::from_str("Loading preview…"));
        files.empty.setHidden(false);
        files.preview_spinner.setHidden(false);
        unsafe { files.preview_spinner.startAnimation(None) };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadWorkspaceFile {
            workspace_id,
            handle,
            path,
            request_id,
            cancellation,
        }) {
            unsafe { files.preview_spinner.stopAnimation(None) };
            files.preview_spinner.setHidden(true);
            files.empty.setStringValue(&NSString::from_str(&format!(
                "Unable to queue file preview: {error}"
            )));
        }
    }

    fn schedule_workspace_file_save(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.loaded_text_path.borrow().is_none() || !files.text_editable.get() {
            return;
        }
        files.text_dirty.set(true);
        let generation = files.text_edit_generation.get().wrapping_add(1);
        files.text_edit_generation.set(generation);
        files
            .metadata
            .setToolTip(Some(&NSString::from_str("Unsaved changes")));
        self.set_workspace_file_editor_status(Some("Edited"));
        let delegate = MainThreadBound::new(self.retain(), self.mtm());
        let when = DispatchTime::try_from(Duration::from_millis(90))
            .expect("90 milliseconds fits dispatch time");
        let _ = DispatchQueue::main().after(when, move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let delegate = delegate.get(mtm);
            let Some(files) = delegate.ivars().files.get() else {
                return;
            };
            if files.text_edit_generation.get() == generation && files.text_dirty.get() {
                delegate.request_workspace_text_highlights();
                delegate.flush_workspace_file_save();
            }
        });
    }

    fn request_workspace_text_highlights(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(path) = files.loaded_text_path.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        let Some(access) = self
            .ivars()
            .workspace_handle
            .borrow()
            .as_ref()
            .map(|handle| handle.workspace_files())
        else {
            return;
        };
        let text = files.text_buffer.borrow().clone();
        let selection = files.text_selection.get();
        let completion_cursor_utf16 = (selection.length == 0).then_some(selection.location);
        let completion_cursor =
            completion_cursor_utf16.and_then(|offset| crate::text_offsets::byte_offset(&text, offset));
        if let Err(error) = requests.try_send(RepositoryRequest::HighlightWorkspaceText {
            workspace_id,
            access,
            path,
            text,
            completion_cursor,
            completion_cursor_utf16,
            edit_generation: files.text_edit_generation.get(),
            cancellation,
        }) {
            log::debug!("native Files syntax request coalesced error={error}");
        }
    }

    fn flush_workspace_file_save(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if !files.text_dirty.get() || files.text_save_in_progress.get() {
            return;
        }
        let Some(path) = files.loaded_text_path.borrow().clone() else {
            return;
        };
        let Some(expected_signature) = files.loaded_text_signature.borrow().clone() else {
            return;
        };
        let Some(access) = self
            .ivars()
            .workspace_handle
            .borrow()
            .as_ref()
            .map(|handle| handle.workspace_files())
        else {
            return;
        };
        self.request_workspace_file_save_with_retry(
            access,
            path,
            files.text_buffer.borrow().clone(),
            expected_signature,
            files.text_edit_generation.get(),
            true,
        );
    }

    fn request_workspace_file_save_with_retry(
        &self,
        access: Arc<dyn FileAccess>,
        path: FileNodePath,
        text: String,
        expected_signature: FileSignature,
        edit_generation: u64,
        allow_sudo_retry: bool,
    ) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.text_save_in_progress.replace(true) {
            return;
        }
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            files.text_save_in_progress.set(false);
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            files.text_save_in_progress.set(false);
            return;
        };
        self.set_workspace_file_editor_status(Some("Saving…"));
        self.set_page_badge("files", NativePageBadge::Indicator);
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.finish_workspace_file_save(
                &workspace_id,
                access,
                path,
                text,
                expected_signature,
                edit_generation,
                allow_sudo_retry,
                Err("The repository service is unavailable.".to_string()),
            );
            return;
        };
        let completion_access = access.clone();
        let completion_path = path.clone();
        let completion_text = text.clone();
        let completion_signature = expected_signature.clone();
        if let Err(error) = requests.try_send(RepositoryRequest::SaveWorkspaceFile {
            workspace_id: workspace_id.clone(),
            access,
            path,
            text,
            expected_signature,
            edit_generation,
            allow_sudo_retry,
            cancellation,
        }) {
            self.finish_workspace_file_save(
                &workspace_id,
                completion_access,
                completion_path,
                completion_text,
                completion_signature,
                edit_generation,
                allow_sudo_retry,
                Err(format!("Unable to queue file save: {error}")),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_workspace_file_save(
        &self,
        workspace_id: &str,
        access: Arc<dyn FileAccess>,
        path: FileNodePath,
        text: String,
        expected_signature: FileSignature,
        edit_generation: u64,
        allow_sudo_retry: bool,
        result: Result<FileNodeInfo, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        self.set_page_badge("files", NativePageBadge::None);
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.text_save_in_progress.set(false);
        match result {
            Ok(info) => {
                if files.loaded_text_path.borrow().as_ref() == Some(&path) {
                    files
                        .loaded_text_signature
                        .replace(Some(file_signature_from_info(&info)));
                    if files.text_edit_generation.get() == edit_generation {
                        files.text_dirty.set(false);
                        files
                            .metadata
                            .setToolTip(Some(&NSString::from_str("Saved")));
                        self.set_workspace_file_editor_status(Some("Saved"));
                        if let Some(next_path) = files.pending_text_selection.borrow_mut().take()
                            && let Some(next_row) = self
                                .filtered_file_tree_rows()
                                .iter()
                                .position(|candidate| candidate.info.path == next_path)
                        {
                            files.table.selectRowIndexes_byExtendingSelection(
                                &NSIndexSet::indexSetWithIndex(next_row),
                                false,
                            );
                            files.table.scrollRowToVisible(next_row as isize);
                        }
                    } else {
                        self.flush_workspace_file_save();
                    }
                }
                log::debug!(
                    "native Files text saved workspace={} path={} generation={}",
                    workspace_id,
                    path.display(),
                    edit_generation
                );
            }
            Err(error) => {
                if allow_sudo_retry && craic_system::system::is_permission_denied_message(&error) {
                    self.offer_file_sudo_retry(
                        access,
                        NativeSudoRetry::Save {
                            path,
                            text,
                            expected_signature,
                            edit_generation,
                        },
                        "Save Failed",
                        &error,
                    );
                    return;
                }
                files.text_dirty.set(true);
                files.pending_text_selection.borrow_mut().take();
                files
                    .metadata
                    .setToolTip(Some(&NSString::from_str("Save failed")));
                self.set_workspace_file_editor_status(Some("Save failed"));
                self.present_path_action_error("Save Failed", &error);
                log::warn!(
                    "native Files text save failed workspace={workspace_id} path={}: {error}",
                    path.display()
                );
            }
        }
    }

    fn apply_workspace_folder(
        &self,
        workspace_id: &str,
        path: &FileNodePath,
        request_id: u64,
        result: Result<NativeFolderPreview, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.preview_request_id.get() != request_id
            || files.selected_path.borrow().as_ref() != Some(path)
        {
            log::debug!(
                "discarding stale workspace folder preview workspace={workspace_id} path={}",
                path.display()
            );
            return;
        }
        if files.editor_search_visible.get() {
            self.hide_editor_search();
        }
        unsafe { files.preview_spinner.stopAnimation(None) };
        files.preview_spinner.setHidden(true);
        files.preview_scroll.setHidden(true);
        files.preview_code.setHidden(true);
        files.preview_image.setHidden(true);
        files.preview_image.clear_image();
        files.preview_pdf.setHidden(true);
        unsafe { files.preview_pdf.setDocument(None) };
        files.preview_web_mode.set(NativeWebPreviewMode::Hidden);
        files.preview_web.setHidden(true);
        files.preview_divider.setHidden(true);
        unsafe { files.preview_web.stopLoading() };
        self.clear_csv_table_preview();

        match result {
            Ok(preview) => {
                let path_label = if path.display().is_empty() {
                    "Workspace root".to_string()
                } else {
                    path.display()
                };
                let permissions = preview.info.mode.map_or_else(
                    || "Unknown".to_string(),
                    |mode| {
                        let mut symbolic = String::with_capacity(10);
                        symbolic.push(if preview.info.capabilities.listable {
                            'd'
                        } else {
                            '-'
                        });
                        for (mask, character) in [
                            (0o400, 'r'),
                            (0o200, 'w'),
                            (0o100, 'x'),
                            (0o040, 'r'),
                            (0o020, 'w'),
                            (0o010, 'x'),
                            (0o004, 'r'),
                            (0o002, 'w'),
                            (0o001, 'x'),
                        ] {
                            symbolic.push(if mode & mask == mask { character } else { '-' });
                        }
                        format!("{symbolic} ({:04o})", mode & 0o777)
                    },
                );
                let modified = preview.info.modified.map_or_else(
                    || "Unknown".to_string(),
                    |modified| {
                        let seconds = modified.duration_since(std::time::UNIX_EPOCH).map_or_else(
                            |error| -error.duration().as_secs_f64(),
                            |duration| duration.as_secs_f64(),
                        );
                        let date = NSDate::dateWithTimeIntervalSince1970(seconds);
                        NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
                            &date,
                            NSDateFormatterStyle::MediumStyle,
                            NSDateFormatterStyle::ShortStyle,
                        )
                        .to_string()
                    },
                );
                let rows = vec![
                    vec!["Workspace Path".to_string(), path_label],
                    vec!["Location".to_string(), preview.provider_path],
                    vec!["Files".to_string(), preview.file_count.to_string()],
                    vec!["Folders".to_string(), preview.folder_count.to_string()],
                    vec![
                        "Total Items".to_string(),
                        (preview.file_count + preview.folder_count).to_string(),
                    ],
                    vec![
                        "Owner".to_string(),
                        preview.info.owner.unwrap_or_else(|| "Unknown".to_string()),
                    ],
                    vec![
                        "Group".to_string(),
                        preview.info.group.unwrap_or_else(|| "Unknown".to_string()),
                    ],
                    vec!["Permissions".to_string(), permissions],
                    vec![
                        "Size".to_string(),
                        preview.info.len.map_or_else(
                            || "Unknown".to_string(),
                            |bytes| format!("{bytes} bytes"),
                        ),
                    ],
                    vec!["Last Modified".to_string(), modified],
                ];
                let content_width = files.preview_table_scroll.contentSize().width.max(320.0);
                for (index, (title, width)) in
                    [("Property", 170.0), ("Value", content_width - 170.0)]
                        .into_iter()
                        .enumerate()
                {
                    let column = NSTableColumn::initWithIdentifier(
                        NSTableColumn::alloc(self.mtm()),
                        &NSUserInterfaceItemIdentifier::from_str(&format!("folder.{index}")),
                    );
                    column.setTitle(&NSString::from_str(title));
                    column.setWidth(width.max(120.0));
                    column.setMinWidth(100.0);
                    files.preview_table.addTableColumn(&column);
                }
                files
                    .preview_table_columns
                    .replace(vec!["Property".to_string(), "Value".to_string()]);
                files.preview_table_rows.replace(rows);
                files.preview_table.setFrameSize(NSSize::new(
                    content_width,
                    (files.preview_table_rows.borrow().len() as f64 * 27.0)
                        .max(files.preview_table_scroll.contentSize().height),
                ));
                files.preview_table.reloadData();
                files
                    .preview_table_scroll
                    .setBorderType(NSBorderType::NoBorder);
                files.preview_table_scroll.setHidden(false);
                files.empty.setHidden(true);
                self.set_workspace_file_editor_status(Some(&format!(
                    "{} files · {} folders",
                    preview.file_count, preview.folder_count
                )));
            }
            Err(error) => {
                files.empty.setStringValue(&NSString::from_str(&format!(
                    "Unable to load folder details: {error}"
                )));
                files.empty.setHidden(false);
            }
        }
    }

    fn clear_font_preview(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.font_registration.borrow().is_some() {
            files.suppress_text_change.set(true);
            files.preview_text.setString(&NSString::new());
            files.suppress_text_change.set(false);
            files
                .preview_text
                .setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(12.0, 0.0)));
            files
                .preview_text
                .setTextContainerInset(NSSize::new(10.0, 10.0));
            files.font_registration.borrow_mut().take();
        }
    }

    fn apply_font_preview(&self, bytes: &[u8]) -> Result<String, String> {
        let Some(files) = self.ivars().files.get() else {
            return Err("The Files preview is unavailable.".to_string());
        };
        let length = isize::try_from(bytes.len())
            .map_err(|_| "This font is too large for Core Graphics.".to_string())?;
        // SAFETY: CFData copies the valid byte slice before this function returns.
        let data = unsafe { CFData::new(None, bytes.as_ptr(), length) }
            .ok_or_else(|| "macOS could not create font data.".to_string())?;
        // SAFETY: Core Text receives immutable CFData containing the complete font bytes.
        let descriptor = unsafe { CTFontManagerCreateFontDescriptorFromData(&data) }
            .ok_or_else(|| "macOS could not decode this font.".to_string())?;
        // SAFETY: Core Text receives the same valid immutable font data.
        let descriptors = unsafe { CTFontManagerCreateFontDescriptorsFromData(&data) };
        if descriptors.count() == 0 {
            return Err("macOS could not find a font face in this file.".to_string());
        }
        // SAFETY: This CFArray was produced by Core Text and contains font descriptors.
        unsafe {
            CTFontManagerRegisterFontDescriptors(
                &descriptors,
                CTFontManagerScope::Process,
                true,
                None,
            );
        }
        let registration = NativeFontRegistration { descriptors };
        // SAFETY: The descriptor is valid and a null matrix requests the identity transform.
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
        append_line("", 14.0);
        append_line("abcdefghijklmnopqrstuvwxyz", 26.0);
        append_line("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 26.0);
        append_line("0123456789 .:,;(*!?')", 26.0);
        append_line("", 14.0);
        for size in [12.0, 18.0, 24.0, 36.0, 48.0, 72.0, 96.0] {
            append_line("Sphinx of black quartz, judge my vow.", size);
        }

        files.suppress_text_change.set(true);
        files.preview_text.setString(&NSString::from_str(&content));
        files.suppress_text_change.set(false);
        let Some(storage) = (unsafe { files.preview_text.textStorage() }) else {
            return Err("macOS could not create font preview storage.".to_string());
        };
        let font_attribute = unsafe { NSFontAttributeName };
        storage.beginEditing();
        for (range, size) in ranges {
            let Some(preview_font) =
                NSFont::fontWithName_size(&NSString::from_str(&post_script_name), size)
            else {
                storage.endEditing();
                return Err(format!("macOS could not activate {display_name}."));
            };
            unsafe { storage.addAttribute_value_range(font_attribute, &preview_font, range) };
        }
        storage.endEditing();
        files.preview_text.setEditable(false);
        files.preview_text.setContinuousSpellCheckingEnabled(false);
        files
            .preview_text
            .setTextContainerInset(NSSize::new(24.0, 24.0));
        files.preview_scroll.setHidden(false);
        files.empty.setHidden(true);
        files.font_registration.replace(Some(registration));
        self.layout_files_preview();
        Ok(display_name)
    }

    fn apply_workspace_file(
        &self,
        workspace_id: &str,
        path: &FileNodePath,
        request_id: u64,
        result: Result<FileRead, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.preview_request_id.get() != request_id
            || files.selected_path.borrow().as_ref() != Some(path)
        {
            log::debug!(
                "discarding stale workspace file preview workspace={workspace_id} path={} request_id={} current_request_id={} selected_path={:?}",
                path.display(),
                request_id,
                files.preview_request_id.get(),
                files
                    .selected_path
                    .borrow()
                    .as_ref()
                    .map(FileNodePath::display)
            );
            return;
        }
        if files.editor_search_visible.get() {
            self.hide_editor_search();
        }
        unsafe { files.preview_spinner.stopAnimation(None) };
        files.preview_spinner.setHidden(true);
        files.preview_scroll.setHidden(true);
        files.preview_code.setHidden(true);
        files.preview_image.setHidden(true);
        files.preview_image.clear_image();
        files.preview_pdf.setHidden(true);
        // SAFETY: Preview completions are applied on AppKit's main thread.
        unsafe { files.preview_pdf.setDocument(None) };
        self.clear_csv_table_preview();
        files.preview_web_mode.set(NativeWebPreviewMode::Hidden);
        files.preview_web.setHidden(true);
        files.preview_divider.setHidden(true);
        // SAFETY: Preview completions are applied on AppKit's main thread.
        unsafe { files.preview_web.stopLoading() };
        self.clear_font_preview();
        self.clear_sqlite_preview();
        files.loaded_text_path.borrow_mut().take();
        files.loaded_text_signature.borrow_mut().take();
        files.text_buffer.borrow_mut().clear();
        files.text_selection.set(NSRange::new(0, 0));
        files.text_editable.set(false);
        files.preview_code.clear_completions();
        files.text_dirty.set(false);
        files.preview_text.setEditable(false);
        match result {
            Ok(read) => {
                let info = read.info;
                let Some(bytes) = read.bytes else {
                    let limit = if is_font_preview_path(&path.display()) {
                        FONT_CONTENT_PREVIEW_LIMIT
                    } else {
                        FILE_CONTENT_PREVIEW_LIMIT
                    };
                    files.empty.setStringValue(&NSString::from_str(&format!(
                        "Preview unavailable because this file exceeds {} MiB.",
                        limit / (1024 * 1024)
                    )));
                    files.empty.setHidden(false);
                    return;
                };
                let path_display = path.display();
                if has_sqlite_header(&bytes) {
                    self.request_workspace_sqlite(path.clone(), info, Some(bytes));
                    return;
                }
                let is_safetensors = Path::new(&path_display)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("safetensors"));
                if is_safetensors {
                    match metadata_text_from_bytes(&bytes, &path_display) {
                        Ok(metadata) => {
                            files.suppress_text_change.set(true);
                            files.preview_text.setString(&NSString::from_str(&metadata));
                            files.suppress_text_change.set(false);
                            files.preview_text.setEditable(false);
                            files.preview_text.setContinuousSpellCheckingEnabled(false);
                            files.preview_scroll.setHidden(false);
                            files.empty.setHidden(true);
                            self.set_workspace_file_editor_status(Some("Safetensors metadata"));
                            self.layout_files_preview();
                        }
                        Err(error) => {
                            files.suppress_text_change.set(true);
                            files.preview_text.setString(&NSString::new());
                            files.suppress_text_change.set(false);
                            files.empty.setStringValue(&NSString::from_str(&error));
                            files.empty.setHidden(false);
                        }
                    }
                    return;
                }
                if is_font_preview_path(&path_display) {
                    match self.apply_font_preview(&bytes) {
                        Ok(name) => {
                            log::info!(
                                "native Core Text font preview applied path={path_display} name={name} bytes={}",
                                bytes.len()
                            );
                            self.set_workspace_file_editor_status(Some(&format!("Font · {name}")));
                        }
                        Err(error) => {
                            files.suppress_text_change.set(true);
                            files.preview_text.setString(&NSString::new());
                            files.suppress_text_change.set(false);
                            files.empty.setStringValue(&NSString::from_str(&error));
                            files.empty.setHidden(false);
                        }
                    }
                    return;
                }
                if is_image_preview_path(&path_display) {
                    let data = NSData::with_bytes(&bytes);
                    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                        files.preview_image.set_image(&image);
                        files.preview_image.setHidden(false);
                        files.empty.setHidden(true);
                    } else {
                        files.empty.setStringValue(&NSString::from_str(
                            "The image could not be decoded by macOS.",
                        ));
                        files.empty.setHidden(false);
                    }
                    return;
                }
                if is_pdf_preview_path(&path_display) {
                    let data = NSData::with_bytes(&bytes);
                    // SAFETY: PDFKit parses retained immutable NSData on AppKit's main thread and
                    // PDFView retains the resulting document.
                    if let Some(document) =
                        unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
                    {
                        unsafe { files.preview_pdf.setDocument(Some(&document)) };
                        files.preview_pdf.setHidden(false);
                        files.empty.setHidden(true);
                    } else {
                        files.empty.setStringValue(&NSString::from_str(
                            "macOS could not decode this PDF.",
                        ));
                        files.empty.setHidden(false);
                    }
                    return;
                }
                if let Some(mime) = media_preview_mime(&path_display) {
                    let data = NSData::with_bytes(&bytes);
                    let Some(base_url) = NSURL::URLWithString(&NSString::from_str("about:blank"))
                    else {
                        files.empty.setStringValue(&NSString::from_str(
                            "Unable to initialize the media preview.",
                        ));
                        files.empty.setHidden(false);
                        return;
                    };
                    // SAFETY: WebKit copies or retains all arguments, and this completion is
                    // applied on AppKit's main thread.
                    unsafe {
                        files
                            .preview_web
                            .loadData_MIMEType_characterEncodingName_baseURL(
                                &data,
                                &NSString::from_str(mime),
                                &NSString::from_str("utf-8"),
                                &base_url,
                            )
                    };
                    files.preview_web_mode.set(NativeWebPreviewMode::FullPane);
                    self.layout_files_preview();
                    files.empty.setHidden(true);
                    return;
                }
                if bytes.contains(&0) {
                    files.empty.setStringValue(&NSString::from_str(
                        "A native preview is not available for this binary file.",
                    ));
                    files.empty.setHidden(false);
                    return;
                }
                match String::from_utf8(bytes) {
                    Ok(text) => {
                        files.loaded_text_path.replace(Some(path.clone()));
                        files
                            .loaded_text_signature
                            .replace(Some(file_signature_from_info(&info)));
                        files.text_dirty.set(false);
                        files.text_buffer.replace(text.clone());
                        files.text_editable.set(info.capabilities.writable);
                        let pending_selection = self
                            .ivars()
                            .pending_files_path
                            .borrow()
                            .as_deref()
                            .filter(|pending| *pending == path.display())
                            .and_then(|_| self.ivars().pending_files_line.get())
                            .map(|line| {
                                let utf16 = crate::text_offsets::offset_for_line_column(
                                    &text,
                                    line,
                                    self.ivars().pending_files_column.get().unwrap_or(1),
                                );
                                let byte = crate::text_offsets::byte_offset(&text, utf16).unwrap_or(0);
                                (NSRange::new(utf16, 0), byte)
                            });
                        let (selection, selection_byte) =
                            pending_selection.unwrap_or((NSRange::new(0, 0), 0));
                        files.text_selection.set(selection);
                        files.preview_code.set_document(
                            &path_display,
                            text.clone(),
                            Vec::new(),
                            craic_render_skia::EditorSelection {
                                anchor: selection_byte,
                                focus: selection_byte,
                            },
                            info.capabilities.writable,
                            true,
                        );
                        files.preview_scroll.setHidden(true);
                        files.preview_code.setHidden(false);
                        files.empty.setHidden(true);
                        self.request_workspace_text_highlights();
                    }
                    Err(_) => {
                        files.empty.setStringValue(&NSString::from_str(
                            "This file is not valid UTF-8 text.",
                        ));
                        files.empty.setHidden(false);
                    }
                }
            }
            Err(error) => {
                files.empty.setStringValue(&NSString::from_str(&format!(
                    "Unable to preview file: {error}"
                )));
                files.empty.setHidden(false);
                log::warn!(
                    "native workspace file preview failed workspace={workspace_id} path={}: {error}",
                    path.display()
                );
            }
        }
    }

    fn set_workspace_file_editor_status(&self, status: Option<&str>) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let base = files.metadata_base.borrow();
        let value = status
            .filter(|status| !status.is_empty())
            .map_or_else(|| base.clone(), |status| format!("{base} · {status}"));
        files.metadata.setStringValue(&NSString::from_str(&value));
    }

    fn apply_workspace_text_highlights(
        &self,
        workspace_id: &str,
        path: &FileNodePath,
        edit_generation: u64,
        result: Result<NativeTextAnalysis, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.loaded_text_path.borrow().as_ref() != Some(path)
            || files.text_edit_generation.get() != edit_generation
        {
            return;
        }
        let analysis = match result {
            Ok(analysis) => analysis,
            Err(error) => {
                log::debug!(
                    "native Files syntax unavailable path={} error={error}",
                    path.display()
                );
                return;
            }
        };
        let text = files.text_buffer.borrow().clone();
        files.preview_code.set_syntax(analysis.syntax.clone());
        files.preview_code.set_folds(analysis.fold_ranges.clone());
        files
            .preview_code
            .set_diagnostics(analysis.diagnostics.clone());
        files.preview_code.clear_completions();
        if analysis.markdown_lint.is_empty() {
            files.preview_code.setToolTip(None);
        } else {
            let mut rules = analysis
                .markdown_lint
                .iter()
                .filter_map(|issue| issue.rule_name.as_deref())
                .take(4)
                .collect::<Vec<_>>();
            rules.sort_unstable();
            rules.dedup();
            let detail = if rules.is_empty() {
                format!("{} Markdown lint issue(s)", analysis.markdown_lint.len())
            } else {
                format!(
                    "{} Markdown lint issue(s): {}",
                    analysis.markdown_lint.len(),
                    rules.join(", ")
                )
            };
            files
                .preview_code
                .setToolTip(Some(&NSString::from_str(&detail)));
        }
        if let (Some(completion), Some(expected_cursor)) =
            (analysis.completion, analysis.completion_cursor_utf16)
        {
            let selection = files.text_selection.get();
            if selection.length == 0
                && selection.location == expected_cursor
                && let Some(replacement_range) = crate::text_offsets::exact_range_for_bytes(
                    &text,
                    completion.replacement_start,
                    completion.replacement_end,
                )
                && replacement_range.location + replacement_range.length == expected_cursor
            {
                files.preview_code.set_completions(
                    completion
                        .items
                        .into_iter()
                        .map(|item| item.insert_text)
                        .collect(),
                    replacement_range,
                );
            }
        }
        if let Some(result) = analysis.csv_table {
            self.apply_csv_table_preview(result);
            return;
        }
        if let Some(result) = analysis.web_preview {
            let preview = match result {
                Ok(preview) => preview,
                Err(error) => {
                    files.preview_scroll.setHidden(true);
                    files.preview_code.setHidden(true);
                    files.preview_web_mode.set(NativeWebPreviewMode::Hidden);
                    files.preview_web.setHidden(true);
                    files.preview_divider.setHidden(true);
                    files.empty.setStringValue(&NSString::from_str(&error));
                    files.empty.setHidden(false);
                    self.layout_files_preview();
                    return;
                }
            };
            let base_url = self
                .ivars()
                .workspace_handle
                .borrow()
                .as_ref()
                .and_then(|handle| handle.workspace_files().local_path(path))
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .map(|path| {
                    NSURL::fileURLWithPath_isDirectory(
                        &NSString::from_str(&path.to_string_lossy()),
                        true,
                    )
                });
            files
                .markdown_editor_source_offset
                .set(self.current_files_editor_source_offset());
            unsafe {
                files.preview_web.loadHTMLString_baseURL(
                    &NSString::from_str(&preview.html),
                    base_url.as_deref(),
                );
            }
            files.preview_web_mode.set(preview.mode);
            if preview.mode == NativeWebPreviewMode::FullPane {
                files.preview_scroll.setHidden(true);
                files.preview_code.setHidden(true);
                files.preview_text.setEditable(false);
            } else {
                files.preview_scroll.setHidden(true);
                files.preview_code.setHidden(false);
            }
            files.empty.setHidden(true);
        } else {
            files.preview_web_mode.set(NativeWebPreviewMode::Hidden);
        }
        self.layout_files_preview();
    }

}
