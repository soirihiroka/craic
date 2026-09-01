impl AppDelegate {
    fn layout_content(&self) {
        let (Some(content), Some(diff_view), Some(search_panel), Some(spinner), Some(empty)) = (
            self.ivars().content.get(),
            self.ivars().diff_view.get(),
            self.ivars().diff_search_panel.get(),
            self.ivars().diff_spinner.get(),
            self.ivars().content_empty.get(),
        ) else {
            return;
        };
        let bounds = content.bounds();
        let safe = content.safeAreaInsets();
        let usable_height = (bounds.size.height - safe.top - safe.bottom).max(1.0);
        diff_view.setFrame(NSRect::new(
            NSPoint::new(0.0, safe.bottom),
            NSSize::new(bounds.size.width.max(1.0), usable_height),
        ));
        search_panel.setFrameOrigin(NSPoint::new(
            (bounds.size.width - 416.0).max(8.0),
            safe.bottom + usable_height - 58.0,
        ));
        spinner.setFrameOrigin(NSPoint::new(
            bounds.size.width / 2.0 - 12.0,
            safe.bottom + usable_height / 2.0 - 12.0,
        ));
        empty.setFrame(NSRect::new(
            NSPoint::new(0.0, safe.bottom + usable_height / 2.0 - 20.0),
            NSSize::new(bounds.size.width.max(1.0), 40.0),
        ));
        if let (Some(root), Some(title), Some(subtitle), Some(cards)) = (
            self.ivars().content_home_root.get(),
            self.ivars().content_home_title.get(),
            self.ivars().content_home_subtitle.get(),
            self.ivars().content_home_cards.get(),
        ) {
            root.setFrame(NSRect::new(
                NSPoint::new(0.0, safe.bottom),
                NSSize::new(bounds.size.width.max(1.0), usable_height),
            ));
            let card_width = 640.0_f64.min((bounds.size.width - 64.0).max(240.0));
            let card_x = ((bounds.size.width - card_width) / 2.0).max(0.0);
            let top = usable_height - 24.0;
            title.setFrame(NSRect::new(
                NSPoint::new(card_x, top - 32.0),
                NSSize::new(card_width, 32.0),
            ));
            subtitle.setFrame(NSRect::new(
                NSPoint::new(card_x, top - 58.0),
                NSSize::new(card_width, 20.0),
            ));
            let visible_cards = cards
                .iter()
                .filter(|card| !card.isHidden())
                .collect::<Vec<_>>();
            let mut card_top = top - 80.0;
            for card in visible_cards {
                card.setFrame(NSRect::new(
                    NSPoint::new(card_x, card_top - 72.0),
                    NSSize::new(card_width, 72.0),
                ));
                card_top -= 82.0;
            }
        }
        if let Some(history) = self.ivars().history.get() {
            history.content_root.setFrame(NSRect::new(
                NSPoint::new(0.0, safe.bottom),
                NSSize::new(bounds.size.width.max(1.0), usable_height),
            ));
            let content_width = bounds.size.width.max(1.0);
            let header_height = 146.0;
            let files_header_height = 34.0;
            let body_height = (usable_height - header_height).max(1.0);
            let files_width = if content_width >= 520.0 {
                280.0
            } else {
                (content_width * 0.42).clamp(160.0, 280.0)
            }
            .min((content_width - 1.0).max(1.0));

            history.title.setFrame(NSRect::new(
                NSPoint::new(20.0, usable_height - 45.0),
                NSSize::new((content_width - 130.0).max(1.0), 26.0),
            ));
            history.comment.setFrame(NSRect::new(
                NSPoint::new(20.0, usable_height - 88.0),
                NSSize::new((content_width - 40.0).max(1.0), 38.0),
            ));
            let metadata_y = usable_height - 130.0;
            history.avatar.setFrame(NSRect::new(
                NSPoint::new(20.0, metadata_y - 6.0),
                NSSize::new(32.0, 32.0),
            ));
            let stats_width = if content_width >= 500.0 { 118.0 } else { 88.0 };
            history.metadata.setFrame(NSRect::new(
                NSPoint::new(60.0, metadata_y),
                NSSize::new((content_width - 80.0 - stats_width).max(1.0), 20.0),
            ));
            history.added.setFrame(NSRect::new(
                NSPoint::new((content_width - stats_width - 12.0).max(0.0), metadata_y),
                NSSize::new(stats_width / 2.0, 20.0),
            ));
            history.deleted.setFrame(NSRect::new(
                NSPoint::new(
                    (content_width - stats_width / 2.0 - 12.0).max(0.0),
                    metadata_y,
                ),
                NSSize::new(stats_width / 2.0, 20.0),
            ));
            history.copy_hash.setFrameOrigin(NSPoint::new(
                (content_width - 82.0).max(0.0),
                usable_height - 45.0,
            ));
            history.open_remote.setFrameOrigin(NSPoint::new(
                (content_width - 44.0).max(0.0),
                usable_height - 45.0,
            ));
            history.files_scroll.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(files_width, (body_height - files_header_height).max(1.0)),
            ));
            history.file_count.setFrame(NSRect::new(
                NSPoint::new(12.0, (body_height - files_header_height + 7.0).max(0.0)),
                NSSize::new((files_width - 24.0).max(1.0), 20.0),
            ));
            let files_content_width = history.files_scroll.contentSize().width.max(1.0);
            let files_table_frame = history.files_table.frame();
            history.files_table.setFrameSize(NSSize::new(
                files_content_width,
                files_table_frame.size.height,
            ));
            history.files_table.sizeLastColumnToFit();
            let files_clip = history.files_scroll.contentView();
            let files_clip_origin = files_clip.bounds().origin;
            if files_clip_origin.x != 0.0 {
                files_clip.setBoundsOrigin(NSPoint::new(0.0, files_clip_origin.y));
                history.files_scroll.reflectScrolledClipView(&files_clip);
            }
            history.diff.setFrame(NSRect::new(
                NSPoint::new(files_width + 1.0, 0.0),
                NSSize::new((content_width - files_width - 1.0).max(1.0), body_height),
            ));
            history.empty.setFrame(NSRect::new(
                NSPoint::new(files_width + 1.0, body_height / 2.0 - 18.0),
                NSSize::new((content_width - files_width - 1.0).max(1.0), 36.0),
            ));
        }
        if let Some(files) = self.ivars().files.get() {
            files.content_root.setFrame(NSRect::new(
                NSPoint::new(0.0, safe.bottom),
                NSSize::new(bounds.size.width.max(1.0), usable_height),
            ));
            self.layout_files_preview();
        }
        if let Some(containers) = self.ivars().containers.get() {
            let width = bounds.size.width.max(1.0);
            containers.content_root.setFrame(NSRect::new(
                NSPoint::new(0.0, safe.bottom),
                NSSize::new(width, usable_height),
            ));
            containers.title.setFrame(NSRect::new(
                NSPoint::new(24.0, (usable_height - 54.0).max(0.0)),
                NSSize::new((width - 48.0).max(1.0), 28.0),
            ));
            containers.subtitle.setFrame(NSRect::new(
                NSPoint::new(24.0, (usable_height - 80.0).max(0.0)),
                NSSize::new((width - 48.0).max(1.0), 20.0),
            ));
            containers.empty.setFrame(NSRect::new(
                NSPoint::new(40.0, usable_height / 2.0 - 30.0),
                NSSize::new((width - 80.0).max(1.0), 60.0),
            ));
            containers.details_scroll.setFrame(NSRect::new(
                NSPoint::new(20.0, 20.0),
                NSSize::new((width - 40.0).max(1.0), (usable_height - 154.0).max(1.0)),
            ));
            containers.inspect_code.setFrame(NSRect::new(
                NSPoint::ZERO,
                NSSize::new(width, (usable_height - 88.0).max(1.0)),
            ));
        }
        if let Some(agents) = self.ivars().agents.get() {
            agents.content_root.setFrame(NSRect::new(
                NSPoint::new(0.0, safe.bottom),
                NSSize::new(bounds.size.width.max(1.0), usable_height),
            ));
            let width = bounds.size.width.max(1.0);
            agents.terminal_panel.setFrame(NSRect::new(
                NSPoint::ZERO,
                NSSize::new(width, usable_height),
            ));
            agents.terminal_stack.setFrame(NSRect::new(
                NSPoint::ZERO,
                NSSize::new(width, usable_height),
            ));
            agents.title.setFrame(NSRect::new(
                NSPoint::new(22.0, usable_height - 48.0),
                NSSize::new((width - 124.0).max(1.0), 24.0),
            ));
            agents.tools.setFrame(NSRect::new(
                NSPoint::new((width - 96.0).max(0.0), usable_height - 51.0),
                NSSize::new(36.0_f64.min(width.max(1.0)), 28.0),
            ));
            agents.thread_actions.setFrame(NSRect::new(
                NSPoint::new((width - 58.0).max(0.0), usable_height - 51.0),
                NSSize::new(36.0_f64.min(width.max(1.0)), 28.0),
            ));
            agents
                .spinner
                .setFrameOrigin(NSPoint::new(22.0, usable_height - 72.0));
            agents.status.setFrame(NSRect::new(
                NSPoint::new(46.0, usable_height - 72.0),
                NSSize::new((width - 344.0).max(1.0), 18.0),
            ));
            agents.usage.setFrame(NSRect::new(
                NSPoint::new((width - 284.0).max(0.0), usable_height - 72.0),
                NSSize::new(168.0_f64.min(width.max(1.0)), 18.0),
            ));
            agents.usage_progress.setFrame(NSRect::new(
                NSPoint::new((width - 108.0).max(0.0), usable_height - 67.0),
                NSSize::new(84.0_f64.min(width.max(1.0)), 8.0),
            ));
            let selector_width = (width - 44.0).max(1.0);
            let controls_width = (selector_width - 16.0).max(1.0);
            let model_width = (controls_width * 0.46).max(1.0);
            let reasoning_width = (controls_width * 0.22).max(1.0);
            let permission_width = (controls_width - model_width - reasoning_width).max(1.0);
            agents.model.setFrame(NSRect::new(
                NSPoint::new(22.0, usable_height - 110.0),
                NSSize::new(model_width, 28.0),
            ));
            agents.reasoning.setFrame(NSRect::new(
                NSPoint::new(30.0 + model_width, usable_height - 110.0),
                NSSize::new(reasoning_width, 28.0),
            ));
            agents.permissions.setFrame(NSRect::new(
                NSPoint::new(38.0 + model_width + reasoning_width, usable_height - 110.0),
                NSSize::new(permission_width, 28.0),
            ));
            let secondary_width = ((selector_width - 8.0) / 2.0).max(1.0);
            agents.personality.setFrame(NSRect::new(
                NSPoint::new(22.0, usable_height - 146.0),
                NSSize::new(secondary_width, 28.0),
            ));
            agents.service_tier.setFrame(NSRect::new(
                NSPoint::new(30.0 + secondary_width, usable_height - 146.0),
                NSSize::new(secondary_width, 28.0),
            ));
            let composer_height = 72.0;
            let composer_bottom = 18.0;
            let composer_width = (width - 156.0).max(120.0);
            agents.composer_scroll.setFrame(NSRect::new(
                NSPoint::new(22.0, composer_bottom),
                NSSize::new(composer_width, composer_height),
            ));
            agents.send.setFrame(NSRect::new(
                NSPoint::new(width - 116.0, composer_bottom + 34.0),
                NSSize::new(92.0, 32.0),
            ));
            agents.stop.setFrame(NSRect::new(
                NSPoint::new(width - 86.0, composer_bottom + 8.0),
                NSSize::new(32.0, 32.0),
            ));
            let attachment_y = composer_bottom + composer_height + 8.0;
            agents.attach.setFrame(NSRect::new(
                NSPoint::new(22.0, attachment_y),
                NSSize::new(30.0, 30.0),
            ));
            agents.attachment_tokens.setFrame(NSRect::new(
                NSPoint::new(60.0, attachment_y + 2.0),
                NSSize::new((width - 116.0).max(1.0), 26.0),
            ));
            agents.clear_attachments.setFrame(NSRect::new(
                NSPoint::new(width - 50.0, attachment_y + 4.0),
                NSSize::new(22.0, 22.0),
            ));
            agents.separator.setFrame(NSRect::new(
                NSPoint::new(0.0, attachment_y + 38.0),
                NSSize::new(width, 1.0),
            ));
            let transcript_bottom = attachment_y + 39.0;
            let transcript_top = usable_height - 158.0;
            agents.transcript_scroll.setFrame(NSRect::new(
                NSPoint::new(8.0, transcript_bottom),
                NSSize::new(
                    (width - 16.0).max(1.0),
                    (transcript_top - transcript_bottom).max(1.0),
                ),
            ));
            let transcript_count = agents.transcript_items.borrow().len();
            if transcript_count != 0 {
                agents.transcript_table.noteHeightOfRowsWithIndexesChanged(
                    &NSIndexSet::indexSetWithIndexesInRange(NSRange::new(0, transcript_count)),
                );
            }
            agents.empty.setFrame(NSRect::new(
                NSPoint::new(28.0, usable_height / 2.0 - 24.0),
                NSSize::new((width - 56.0).max(1.0), 48.0),
            ));
            self.layout_native_agent_terminal_panel();
        }
    }

    fn layout_files_preview(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let bounds = files.content_root.bounds();
        let frame = NSRect::new(
            NSPoint::new(0.0, 20.0),
            NSSize::new(
                bounds.size.width.max(1.0),
                (bounds.size.height - 112.0).max(1.0),
            ),
        );
        files.preview_image.setFrame(frame);
        files.preview_image.recalculate_fit();
        files.preview_pdf.setFrame(frame);
        if files.sqlite_controls.isHidden() {
            files.preview_table_scroll.setFrame(frame);
        } else {
            let controls_y = frame.origin.y + frame.size.height - SQLITE_CONTROLS_HEIGHT;
            files.sqlite_controls.setFrame(NSRect::new(
                NSPoint::new(frame.origin.x, controls_y),
                NSSize::new(frame.size.width, SQLITE_CONTROLS_HEIGHT),
            ));
            files.preview_table_scroll.setFrame(NSRect::new(
                frame.origin,
                NSSize::new(
                    frame.size.width,
                    (frame.size.height - SQLITE_CONTROLS_HEIGHT - 8.0).max(1.0),
                ),
            ));
            let width = frame.size.width;
            let table_width = 176.0;
            let column_width = 132.0;
            let button_width = 30.0;
            let trailing_width = 196.0;
            let filter_width =
                (width - table_width - column_width - trailing_width - 24.0).max(100.0);
            files.sqlite_table_selector.setFrame(NSRect::new(
                NSPoint::new(0.0, 2.0),
                NSSize::new(table_width, 28.0),
            ));
            files.sqlite_column_selector.setFrame(NSRect::new(
                NSPoint::new(table_width + 6.0, 2.0),
                NSSize::new(column_width, 28.0),
            ));
            files.sqlite_filter.setFrame(NSRect::new(
                NSPoint::new(table_width + column_width + 12.0, 2.0),
                NSSize::new(filter_width, 28.0),
            ));
            let trailing_x = width - trailing_width;
            files.sqlite_status.setFrame(NSRect::new(
                NSPoint::new(trailing_x, 7.0),
                NSSize::new(100.0, 18.0),
            ));
            files.sqlite_previous.setFrame(NSRect::new(
                NSPoint::new(width - button_width * 3.0 - 2.0, 2.0),
                NSSize::new(button_width, 28.0),
            ));
            files.sqlite_next.setFrame(NSRect::new(
                NSPoint::new(width - button_width * 2.0, 2.0),
                NSSize::new(button_width, 28.0),
            ));
            files.sqlite_reload.setFrame(NSRect::new(
                NSPoint::new(width - button_width, 2.0),
                NSSize::new(button_width, 28.0),
            ));
        }
        match files.preview_web_mode.get() {
            NativeWebPreviewMode::BesideEditor => {
                let editor_width = ((frame.size.width - 1.0) / 2.0).max(1.0);
                let search_height = if files.editor_search_visible.get() {
                    54.0
                } else {
                    0.0
                };
                let editor_height = (frame.size.height - search_height).max(1.0);
                files.preview_scroll.setFrame(NSRect::new(
                    frame.origin,
                    NSSize::new(editor_width, editor_height),
                ));
                files.preview_code.setFrame(NSRect::new(
                    frame.origin,
                    NSSize::new(editor_width, editor_height),
                ));
                files.editor_search_panel.setFrameOrigin(NSPoint::new(
                    frame.origin.x + ((editor_width - 400.0) / 2.0).max(0.0),
                    frame.origin.y + editor_height + 4.0,
                ));
                files
                    .editor_search_panel
                    .setHidden(!files.editor_search_visible.get());
                files.preview_divider.setFrame(NSRect::new(
                    NSPoint::new(frame.origin.x + editor_width, frame.origin.y),
                    NSSize::new(1.0, frame.size.height),
                ));
                files.preview_web.setFrame(NSRect::new(
                    NSPoint::new(frame.origin.x + editor_width + 1.0, frame.origin.y),
                    NSSize::new(
                        (frame.size.width - editor_width - 1.0).max(1.0),
                        frame.size.height,
                    ),
                ));
                files.preview_divider.setHidden(false);
                files.preview_web.setHidden(false);
            }
            NativeWebPreviewMode::FullPane => {
                files.editor_search_panel.setHidden(true);
                files.preview_scroll.setFrame(frame);
                files.preview_code.setFrame(frame);
                files.preview_code.setHidden(true);
                files.preview_divider.setHidden(true);
                files.preview_web.setFrame(frame);
                files.preview_web.setHidden(false);
            }
            NativeWebPreviewMode::Hidden => {
                let search_height = if files.editor_search_visible.get() {
                    54.0
                } else {
                    0.0
                };
                let editor_height = (frame.size.height - search_height).max(1.0);
                let editor_frame =
                    NSRect::new(frame.origin, NSSize::new(frame.size.width, editor_height));
                files.preview_scroll.setFrame(editor_frame);
                files.preview_code.setFrame(editor_frame);
                files.editor_search_panel.setFrameOrigin(NSPoint::new(
                    frame.origin.x + ((frame.size.width - 400.0) / 2.0).max(0.0),
                    frame.origin.y + editor_height + 4.0,
                ));
                files
                    .editor_search_panel
                    .setHidden(!files.editor_search_visible.get());
                files.preview_divider.setHidden(true);
                files.preview_web.setHidden(true);
            }
        }
    }

    fn current_files_editor_source_offset(&self) -> Option<usize> {
        let files = self.ivars().files.get()?;
        Some(files.preview_code.preview_source_offset())
    }

    fn scroll_files_markdown_preview_to_source_offset(&self, source_offset: usize) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let script =
            format!("window.CraicMarkdownPreview?.scrollToSourceOffset?.({source_offset}, false);");
        // SAFETY: The bundled bridge is the only JavaScript executed by this preview. The
        // completion result is intentionally ignored because a new document may replace it.
        unsafe {
            files
                .preview_web
                .evaluateJavaScript_completionHandler(&NSString::from_str(&script), None);
        }
    }

    pub(crate) fn files_code_scroll_changed(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.preview_web_mode.get() != NativeWebPreviewMode::BesideEditor {
            return;
        }
        let source_offset = files.preview_code.preview_source_offset();
        if files
            .markdown_editor_source_offset
            .replace(Some(source_offset))
            != Some(source_offset)
        {
            self.scroll_files_markdown_preview_to_source_offset(source_offset);
        }
    }

    pub(crate) fn files_code_text_changed(&self, text: String, selection: NSRange) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.text_buffer.replace(text);
        files.text_selection.set(selection);
        files.preview_code.clear_completions();
        self.schedule_workspace_file_save();
    }

    pub(crate) fn files_code_selection_changed(&self, selection: NSRange) {
        if let Some(files) = self.ivars().files.get() {
            files.text_selection.set(selection);
        }
    }

}
