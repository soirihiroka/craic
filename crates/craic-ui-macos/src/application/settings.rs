impl AppDelegate {
    fn show_commit_message_settings(&self) {
        if self.ivars().commit_message_settings.get().is_none() {
            let settings = self.make_commit_message_settings();
            self.ivars()
                .commit_message_settings
                .set(settings)
                .unwrap_or_else(|_| panic!("commit-message settings UI is initialized once"));
        }
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        let font_sizes = craic_config::load().font_sizes;
        self.ivars().font_sizes.set(font_sizes);
        Self::populate_native_font_size_fields(settings, font_sizes);
        settings.font_status.setStringValue(&NSString::new());
        settings.window.center();
        settings.window.makeKeyAndOrderFront(Some(self));
        settings.provider.setEnabled(false);
        settings.model.setEnabled(false);
        settings
            .status
            .setStringValue(&NSString::from_str("Loading commit-message settings…"));
        settings.spinner.setHidden(false);
        unsafe { settings.spinner.startAnimation(None) };
        let request_id = settings.request_id.get().wrapping_add(1);
        settings.request_id.set(request_id);
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.set_commit_message_settings_error("The settings service is unavailable.");
            return;
        };
        if let Err(error) =
            requests.try_send(RepositoryRequest::LoadCommitMessageSettings { request_id })
        {
            self.set_commit_message_settings_error(&format!(
                "Unable to load commit-message settings: {error}"
            ));
        }
        self.request_workspace_settings_load();
    }

    fn make_commit_message_settings(&self) -> CommitMessageSettingsUi {
        let mtm = self.mtm();
        let size = NSSize::new(560.0, 438.0);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::ZERO, size),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Craic Settings"));
        window.setToolbarStyle(NSWindowToolbarStyle::Preference);

        let ai_pane = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::ZERO, size));
        let title = NSTextField::labelWithString(&NSString::from_str("AI Commit Messages"), mtm);
        title.setFrame(NSRect::new(
            NSPoint::new(24.0, 390.0),
            NSSize::new(512.0, 26.0),
        ));
        title.setFont(Some(&NSFont::boldSystemFontOfSize(18.0)));
        ai_pane.addSubview(&title);

        let description = NSTextField::wrappingLabelWithString(
            &NSString::from_str(
                "Choose the provider and model used by the commit composer’s magic-wand action.",
            ),
            mtm,
        );
        description.setFrame(NSRect::new(
            NSPoint::new(24.0, 356.0),
            NSSize::new(512.0, 32.0),
        ));
        description.setTextColor(Some(&NSColor::secondaryLabelColor()));
        ai_pane.addSubview(&description);

        let provider_label = NSTextField::labelWithString(&NSString::from_str("Provider"), mtm);
        provider_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 318.0),
            NSSize::new(92.0, 24.0),
        ));
        ai_pane.addSubview(&provider_label);
        let provider = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(122.0, 314.0), NSSize::new(414.0, 30.0)),
            false,
        );
        let providers = registered_providers();
        let provider_ids = providers
            .iter()
            .map(|provider| provider.id().to_string())
            .collect::<Vec<_>>();
        for provider_option in providers {
            provider.addItemWithTitle(&NSString::from_str(provider_option.label()));
        }
        unsafe {
            provider.setTarget(Some(self));
            provider.setAction(Some(sel!(commitMessageProviderChanged:)));
        }
        provider.setEnabled(false);
        ai_pane.addSubview(&provider);

        let model_label = NSTextField::labelWithString(&NSString::from_str("Model"), mtm);
        model_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 278.0),
            NSSize::new(92.0, 24.0),
        ));
        ai_pane.addSubview(&model_label);
        let model = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(122.0, 274.0), NSSize::new(380.0, 30.0)),
            false,
        );
        model.addItemWithTitle(&NSString::from_str("Loading…"));
        unsafe {
            model.setTarget(Some(self));
            model.setAction(Some(sel!(commitMessageModelChanged:)));
        }
        model.setEnabled(false);
        ai_pane.addSubview(&model);

        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(NSPoint::new(512.0, 280.0), NSSize::new(16.0, 16.0)),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setIndeterminate(true);
        spinner.setDisplayedWhenStopped(false);
        spinner.setHidden(true);
        ai_pane.addSubview(&spinner);

        let status = NSTextField::wrappingLabelWithString(&NSString::new(), mtm);
        status.setFrame(NSRect::new(
            NSPoint::new(24.0, 237.0),
            NSSize::new(512.0, 32.0),
        ));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setMaximumNumberOfLines(2);
        ai_pane.addSubview(&status);

        let appearance_pane =
            NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::ZERO, size));
        let font_title = NSTextField::labelWithString(&NSString::from_str("Font Sizes"), mtm);
        font_title.setFrame(NSRect::new(
            NSPoint::new(24.0, 390.0),
            NSSize::new(220.0, 26.0),
        ));
        font_title.setFont(Some(&NSFont::boldSystemFontOfSize(18.0)));
        appearance_pane.addSubview(&font_title);

        let font_labels = ["Shell", "Text Editor", "Diff", "Agents Chat"];
        let font_x = [24.0, 152.0, 280.0, 408.0];
        let mut font_fields = Vec::with_capacity(font_labels.len());
        for (label, x) in font_labels.into_iter().zip(font_x) {
            let label = NSTextField::labelWithString(&NSString::from_str(label), mtm);
            label.setFrame(NSRect::new(
                NSPoint::new(x, 350.0),
                NSSize::new(112.0, 18.0),
            ));
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            appearance_pane.addSubview(&label);

            let field = NSTextField::initWithFrame(
                NSTextField::alloc(mtm),
                NSRect::new(NSPoint::new(x, 318.0), NSSize::new(104.0, 28.0)),
            );
            field.setAlignment(NSTextAlignment::Right);
            appearance_pane.addSubview(&field);
            font_fields.push(field);
        }
        let [
            shell_font_size,
            editor_font_size,
            diff_font_size,
            agent_font_size,
        ] = font_fields
            .try_into()
            .expect("four font fields are created");

        let font_status = NSTextField::labelWithString(&NSString::new(), mtm);
        font_status.setFrame(NSRect::new(
            NSPoint::new(24.0, 278.0),
            NSSize::new(342.0, 18.0),
        ));
        font_status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        font_status.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        appearance_pane.addSubview(&font_status);

        let save_fonts = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Apply Font Sizes"),
                Some(self),
                Some(sel!(saveFontSizes:)),
                mtm,
            )
        };
        save_fonts.setFrame(NSRect::new(
            NSPoint::new(390.0, 270.0),
            NSSize::new(146.0, 32.0),
        ));
        save_fonts.setBezelStyle(NSBezelStyle::Push);
        appearance_pane.addSubview(&save_fonts);

        let workspace_section = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(560.0, 438.0)),
        );
        let workspace_title =
            NSTextField::labelWithString(&NSString::from_str("Workspace Git"), mtm);
        workspace_title.setFrame(NSRect::new(
            NSPoint::new(24.0, 398.0),
            NSSize::new(400.0, 26.0),
        ));
        workspace_title.setFont(Some(&NSFont::boldSystemFontOfSize(18.0)));
        workspace_section.addSubview(&workspace_title);

        let workspace_spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(NSPoint::new(516.0, 404.0), NSSize::new(16.0, 16.0)),
        );
        workspace_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        workspace_spinner.setControlSize(NSControlSize::Small);
        workspace_spinner.setIndeterminate(true);
        workspace_spinner.setDisplayedWhenStopped(false);
        workspace_section.addSubview(&workspace_spinner);

        let use_global_user = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str("Use global Git author for this workspace"),
                Some(self),
                Some(sel!(workspaceUseGlobalChanged:)),
                mtm,
            )
        };
        use_global_user.setFrame(NSRect::new(
            NSPoint::new(24.0, 354.0),
            NSSize::new(512.0, 24.0),
        ));
        workspace_section.addSubview(&use_global_user);

        let author_name_label =
            NSTextField::labelWithString(&NSString::from_str("Author Name"), mtm);
        author_name_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 316.0),
            NSSize::new(112.0, 22.0),
        ));
        workspace_section.addSubview(&author_name_label);
        let author_name = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            NSRect::new(NSPoint::new(142.0, 312.0), NSSize::new(394.0, 28.0)),
        );
        author_name.setPlaceholderString(Some(&NSString::from_str("Git author name")));
        workspace_section.addSubview(&author_name);

        let author_email_label =
            NSTextField::labelWithString(&NSString::from_str("Author Email"), mtm);
        author_email_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 278.0),
            NSSize::new(112.0, 22.0),
        ));
        workspace_section.addSubview(&author_email_label);
        let author_email = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            NSRect::new(NSPoint::new(142.0, 274.0), NSSize::new(394.0, 28.0)),
        );
        author_email.setPlaceholderString(Some(&NSString::from_str("Git author email")));
        workspace_section.addSubview(&author_email);

        let timezone_label =
            NSTextField::labelWithString(&NSString::from_str("Commit Timezone"), mtm);
        timezone_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 240.0),
            NSSize::new(112.0, 22.0),
        ));
        workspace_section.addSubview(&timezone_label);
        let commit_timezone = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            NSRect::new(NSPoint::new(142.0, 236.0), NSSize::new(394.0, 28.0)),
        );
        commit_timezone.setPlaceholderString(Some(&NSString::from_str("+0000 or +09:30")));
        commit_timezone.setToolTip(Some(&NSString::from_str(
            "Use +0000, -0500, or +09:30. Leave empty for the default.",
        )));
        workspace_section.addSubview(&commit_timezone);

        let use_system_timezone = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str("Use system timezone when no commit timezone is set"),
                None,
                None,
                mtm,
            )
        };
        use_system_timezone.setFrame(NSRect::new(
            NSPoint::new(24.0, 198.0),
            NSSize::new(512.0, 24.0),
        ));
        workspace_section.addSubview(&use_system_timezone);

        let remote_owner_warning = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str("Warn when the Git author differs from the remote owner"),
                None,
                None,
                mtm,
            )
        };
        remote_owner_warning.setFrame(NSRect::new(
            NSPoint::new(24.0, 164.0),
            NSSize::new(512.0, 24.0),
        ));
        workspace_section.addSubview(&remote_owner_warning);

        let github_label = NSTextField::labelWithString(&NSString::from_str("GitHub Account"), mtm);
        github_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 124.0),
            NSSize::new(112.0, 22.0),
        ));
        workspace_section.addSubview(&github_label);
        let github_account = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(142.0, 120.0), NSSize::new(394.0, 28.0)),
            false,
        );
        github_account.addItemWithTitle(&NSString::from_str("Loading GitHub accounts…"));
        github_account.setEnabled(false);
        workspace_section.addSubview(&github_account);

        let save_workspace = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Save Workspace Settings"),
                Some(self),
                Some(sel!(saveWorkspaceSettings:)),
                mtm,
            )
        };
        save_workspace.setFrame(NSRect::new(
            NSPoint::new(350.0, 70.0),
            NSSize::new(186.0, 32.0),
        ));
        save_workspace.setBezelStyle(NSBezelStyle::Push);
        save_workspace.setEnabled(false);
        workspace_section.addSubview(&save_workspace);

        let workspace_status = NSTextField::wrappingLabelWithString(&NSString::new(), mtm);
        workspace_status.setFrame(NSRect::new(
            NSPoint::new(24.0, 20.0),
            NSSize::new(512.0, 42.0),
        ));
        workspace_status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        workspace_status.setMaximumNumberOfLines(2);
        workspace_section.addSubview(&workspace_status);
        let tab_controller = NSTabViewController::new(mtm);
        tab_controller.setTabStyle(NSTabViewControllerTabStyle::Toolbar);
        tab_controller.setCanPropagateSelectedChildViewControllerTitle(false);
        for (label, symbol, pane) in [
            ("AI", "wand.and.stars", &ai_pane),
            ("Appearance", "textformat.size", &appearance_pane),
            ("Workspace Git", "arrow.triangle.branch", &workspace_section),
        ] {
            let controller = NSViewController::new(mtm);
            controller.setView(pane);
            controller.setPreferredContentSize(size);
            controller.setTitle(Some(&NSString::from_str(label)));
            let item = NSTabViewItem::tabViewItemWithViewController(&controller);
            item.setLabel(&NSString::from_str(label));
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(label)),
            ) {
                item.setImage(Some(&image));
            }
            tab_controller.addTabViewItem(&item);
        }
        tab_controller.setSelectedTabViewItemIndex(0);
        window.setContentViewController(Some(&tab_controller));

        CommitMessageSettingsUi {
            window,
            shell_font_size,
            editor_font_size,
            diff_font_size,
            agent_font_size,
            font_status,
            provider,
            model,
            spinner,
            status,
            provider_ids,
            model_ids: RefCell::new(vec![None]),
            current_provider: RefCell::new(default_provider().id().to_string()),
            request_id: Cell::new(0),
            workspace_section,
            workspace_spinner,
            workspace_status,
            use_global_user,
            author_name,
            author_email,
            commit_timezone,
            use_system_timezone,
            remote_owner_warning,
            github_account,
            github_accounts: RefCell::new(vec![None]),
            workspace_settings: RefCell::new(None),
            save_workspace,
            workspace_loading: Cell::new(false),
            workspace_request_id: Cell::new(0),
        }
    }

    fn populate_native_font_size_fields(
        settings: &CommitMessageSettingsUi,
        font_sizes: craic_config::FontSizes,
    ) {
        for (field, value) in [
            (&settings.shell_font_size, font_sizes.shell),
            (&settings.editor_font_size, font_sizes.editor),
            (&settings.diff_font_size, font_sizes.diff),
            (&settings.agent_font_size, font_sizes.agent),
        ] {
            field.setStringValue(&NSString::from_str(&format!("{value:.1}")));
        }
    }

    fn apply_native_font_size_settings(&self) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        let parse = |field: &NSTextField, label: &str, default| {
            let value = field
                .stringValue()
                .to_string()
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("Enter a numeric {label} size."))?;
            if !value.is_finite() {
                return Err(format!("Enter a finite {label} size."));
            }
            Ok(craic_config::normalize_font_size(value, default))
        };
        let font_sizes = match (|| {
            Ok::<_, String>(craic_config::FontSizes {
                shell: parse(
                    &settings.shell_font_size,
                    "Shell",
                    craic_config::DEFAULT_SHELL_FONT_SIZE,
                )?,
                editor: parse(
                    &settings.editor_font_size,
                    "Text Editor",
                    craic_config::DEFAULT_EDITOR_FONT_SIZE,
                )?,
                diff: parse(
                    &settings.diff_font_size,
                    "Diff",
                    craic_config::DEFAULT_DIFF_FONT_SIZE,
                )?,
                agent: parse(
                    &settings.agent_font_size,
                    "Agents Chat",
                    craic_config::DEFAULT_AGENT_FONT_SIZE,
                )?,
            })
        })() {
            Ok(font_sizes) => font_sizes,
            Err(message) => {
                settings
                    .font_status
                    .setTextColor(Some(&NSColor::systemRedColor()));
                settings
                    .font_status
                    .setStringValue(&NSString::from_str(&message));
                return;
            }
        };

        craic_config::save_font_sizes(font_sizes);
        self.ivars().font_sizes.set(font_sizes);
        Self::populate_native_font_size_fields(settings, font_sizes);

        for session in self.ivars().terminal_sessions.borrow().iter() {
            session.view.set_font_size(font_sizes.shell);
        }
        if let Some(diff) = self.ivars().diff_view.get() {
            diff.set_font_size(font_sizes.diff);
        }
        if let Some(history) = self.ivars().history.get() {
            history.diff.set_font_size(font_sizes.diff);
        }
        if let Some(files) = self.ivars().files.get() {
            files
                .preview_text
                .setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
                    font_sizes.editor,
                    0.0,
                )));
            files.preview_code.set_font_size(font_sizes.editor);
        }
        if let Some(agents) = self.ivars().agents.get() {
            agents
                .composer
                .setFont(Some(&NSFont::systemFontOfSize(font_sizes.agent)));
            agents.transcript_table.reloadData();
        }

        settings
            .font_status
            .setTextColor(Some(&NSColor::secondaryLabelColor()));
        settings
            .font_status
            .setStringValue(&NSString::from_str("Saved and applied."));
        log::info!(
            "native font sizes applied shell={} editor={} diff={} agent={}",
            font_sizes.shell,
            font_sizes.editor,
            font_sizes.diff,
            font_sizes.agent
        );
    }

    fn request_workspace_settings_load(&self) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            settings.workspace_section.setHidden(false);
            settings
                .workspace_status
                .setStringValue(&NSString::from_str(
                    "Open a workspace to edit its Git settings.",
                ));
            settings.workspace_settings.borrow_mut().take();
            self.update_workspace_settings_control_state();
            return;
        };
        let workspace = self
            .ivars()
            .workspaces
            .borrow()
            .iter()
            .find(|workspace| workspace.selection_id() == workspace_id)
            .map(|workspace| workspace.workspace.clone());
        let Some(workspace) = workspace else {
            settings.workspace_settings.borrow_mut().take();
            settings
                .workspace_status
                .setStringValue(&NSString::from_str(
                    "The active workspace configuration is unavailable.",
                ));
            self.update_workspace_settings_control_state();
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            settings.workspace_settings.borrow_mut().take();
            settings
                .workspace_status
                .setStringValue(&NSString::from_str(
                    "Git settings are unavailable for this workspace.",
                ));
            self.update_workspace_settings_control_state();
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            settings
                .workspace_status
                .setStringValue(&NSString::from_str(
                    "The workspace settings service is unavailable.",
                ));
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        settings.workspace_loading.set(true);
        settings.workspace_settings.borrow_mut().take();
        let request_id = settings.workspace_request_id.get().wrapping_add(1);
        settings.workspace_request_id.set(request_id);
        settings
            .workspace_status
            .setStringValue(&NSString::from_str("Loading workspace Git settings…"));
        unsafe { settings.workspace_spinner.startAnimation(None) };
        self.update_workspace_settings_control_state();
        if let Err(error) = requests.try_send(RepositoryRequest::LoadWorkspaceSettings {
            workspace_id,
            request_id,
            workspace,
            handle,
            cancellation,
        }) {
            settings.workspace_loading.set(false);
            unsafe { settings.workspace_spinner.stopAnimation(None) };
            settings
                .workspace_status
                .setStringValue(&NSString::from_str(&format!(
                    "Unable to load workspace settings: {error}"
                )));
            self.update_workspace_settings_control_state();
        }
    }

    fn apply_workspace_settings(
        &self,
        workspace_id: &str,
        request_id: u64,
        result: Result<NativeWorkspaceSettings, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(ui) = self.ivars().commit_message_settings.get() else {
            return;
        };
        if ui.workspace_request_id.get() != request_id {
            log::debug!(
                "discarding stale native workspace settings workspace={workspace_id} request={request_id}"
            );
            return;
        }
        ui.workspace_loading.set(false);
        unsafe { ui.workspace_spinner.stopAnimation(None) };
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(error) => {
                ui.workspace_settings.borrow_mut().take();
                ui.workspace_status
                    .setStringValue(&NSString::from_str(&error));
                self.update_workspace_settings_control_state();
                return;
            }
        };
        let settings = loaded.settings;
        ui.use_global_user.setState(if settings.use_global_user {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        ui.author_name.setStringValue(&NSString::from_str(
            settings
                .local_user_name
                .as_deref()
                .or(settings.global_user_name.as_deref())
                .unwrap_or_default(),
        ));
        ui.author_email.setStringValue(&NSString::from_str(
            settings
                .local_user_email
                .as_deref()
                .or(settings.global_user_email.as_deref())
                .unwrap_or_default(),
        ));
        ui.commit_timezone.setStringValue(&NSString::from_str(
            settings.commit_timezone.as_deref().unwrap_or_default(),
        ));
        ui.use_system_timezone
            .setState(if settings.use_system_timezone {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        ui.remote_owner_warning
            .setState(if settings.warn_if_remote_owner_mismatch {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });

        let account_error = loaded.github_accounts.as_ref().err().cloned();
        let mut accounts = loaded.github_accounts.unwrap_or_default();
        accounts.sort_by(|left, right| {
            left.host
                .cmp(&right.host)
                .then_with(|| left.login.cmp(&right.login))
        });
        if let Some(selected) = settings.github_auth_account.as_ref()
            && !accounts.iter().any(|account| account == selected)
        {
            accounts.push(selected.clone());
        }
        let mut choices = vec![None];
        choices.extend(accounts.into_iter().map(Some));
        ui.github_account.removeAllItems();
        ui.github_account
            .addItemWithTitle(&NSString::from_str("Use active gh account"));
        for account in choices.iter().flatten() {
            let label = if account.host.eq_ignore_ascii_case("github.com") {
                account.login.clone()
            } else {
                format!("{} on {}", account.login, account.host)
            };
            ui.github_account
                .addItemWithTitle(&NSString::from_str(&label));
        }
        let selected_index = settings
            .github_auth_account
            .as_ref()
            .and_then(|selected| {
                choices
                    .iter()
                    .position(|choice| choice.as_ref() == Some(selected))
            })
            .unwrap_or_default();
        ui.github_account.selectItemAtIndex(selected_index as isize);
        ui.github_accounts.replace(choices);
        ui.workspace_settings.replace(Some(settings));
        ui.workspace_status.setStringValue(&NSString::from_str(
            account_error.as_deref().unwrap_or(
                "Workspace Git settings are loaded. Changes are saved when you press Save.",
            ),
        ));
        self.update_workspace_settings_control_state();
        log::debug!(
            "native workspace settings applied workspace={workspace_id} request={request_id} account_choices={}",
            ui.github_accounts.borrow().len()
        );
    }

    fn update_workspace_settings_control_state(&self) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        let enabled =
            settings.workspace_settings.borrow().is_some() && !settings.workspace_loading.get();
        settings.use_global_user.setEnabled(enabled);
        let local_author = enabled && settings.use_global_user.state() != NSControlStateValueOn;
        settings.author_name.setEnabled(local_author);
        settings.author_email.setEnabled(local_author);
        settings.commit_timezone.setEnabled(enabled);
        settings.use_system_timezone.setEnabled(enabled);
        settings.remote_owner_warning.setEnabled(enabled);
        settings.github_account.setEnabled(enabled);
        settings.save_workspace.setEnabled(enabled);
    }

    fn request_workspace_settings_save(&self) {
        let Some(ui) = self.ivars().commit_message_settings.get() else {
            return;
        };
        let Some(base) = ui.workspace_settings.borrow().clone() else {
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
        let selected_account = usize::try_from(ui.github_account.indexOfSelectedItem())
            .ok()
            .and_then(|index| ui.github_accounts.borrow().get(index).cloned())
            .flatten();
        let text_option = |field: &NSTextField| {
            let value = field.stringValue().to_string();
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        };
        let settings = GitSettings {
            global_user_name: base.global_user_name,
            global_user_email: base.global_user_email,
            local_user_name: text_option(&ui.author_name),
            local_user_email: text_option(&ui.author_email),
            use_global_user: ui.use_global_user.state() == NSControlStateValueOn,
            commit_timezone: text_option(&ui.commit_timezone),
            warn_if_remote_owner_mismatch: ui.remote_owner_warning.state() == NSControlStateValueOn,
            use_system_timezone: ui.use_system_timezone.state() == NSControlStateValueOn,
            github_auth_account: selected_account,
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        ui.workspace_loading.set(true);
        let request_id = ui.workspace_request_id.get().wrapping_add(1);
        ui.workspace_request_id.set(request_id);
        ui.workspace_status
            .setStringValue(&NSString::from_str("Saving workspace Git settings…"));
        unsafe { ui.workspace_spinner.startAnimation(None) };
        self.update_workspace_settings_control_state();
        if let Err(error) = requests.try_send(RepositoryRequest::SaveWorkspaceSettings {
            workspace_id,
            request_id,
            handle,
            settings,
            cancellation,
        }) {
            ui.workspace_loading.set(false);
            unsafe { ui.workspace_spinner.stopAnimation(None) };
            ui.workspace_status
                .setStringValue(&NSString::from_str(&format!(
                    "Unable to save workspace settings: {error}"
                )));
            self.update_workspace_settings_control_state();
        }
    }

    fn finish_workspace_settings_save(
        &self,
        workspace_id: &str,
        request_id: u64,
        handle: Arc<GitRepoHandle>,
        result: Result<WorkspaceSnapshot, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(ui) = self.ivars().commit_message_settings.get() else {
            return;
        };
        if ui.workspace_request_id.get() != request_id {
            log::debug!(
                "discarding stale native workspace settings save workspace={workspace_id} request={request_id}"
            );
            return;
        }
        ui.workspace_loading.set(false);
        unsafe { ui.workspace_spinner.stopAnimation(None) };
        match result {
            Ok(snapshot) => {
                self.apply_repository_snapshot(workspace_id, Some(handle), None, Ok(snapshot));
                ui.workspace_status
                    .setStringValue(&NSString::from_str("Workspace Git settings saved."));
                self.request_workspace_settings_load();
                log::info!(
                    "native workspace settings saved workspace={workspace_id} request={request_id}"
                );
            }
            Err(error) => {
                ui.workspace_status
                    .setStringValue(&NSString::from_str(&error));
                self.update_workspace_settings_control_state();
            }
        }
    }

    fn request_commit_message_models(&self, provider_id: String, selected_model: Option<String>) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        let request_id = settings.request_id.get().wrapping_add(1);
        settings.request_id.set(request_id);
        settings.model.removeAllItems();
        settings
            .model
            .addItemWithTitle(&NSString::from_str("Loading…"));
        settings.model.setEnabled(false);
        settings.spinner.setHidden(false);
        unsafe { settings.spinner.startAnimation(None) };
        settings
            .status
            .setStringValue(&NSString::from_str("Loading available models…"));
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.set_commit_message_settings_error("The settings service is unavailable.");
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadCommitMessageModels {
            provider_id,
            selected_model,
            request_id,
        }) {
            self.set_commit_message_settings_error(&format!(
                "Unable to load provider models: {error}"
            ));
        }
    }

    fn apply_commit_message_settings(
        &self,
        request_id: u64,
        provider_id: String,
        model: Option<String>,
    ) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        if settings.request_id.get() != request_id {
            return;
        }
        let provider_id = settings
            .provider_ids
            .iter()
            .find(|candidate| **candidate == provider_id)
            .cloned()
            .unwrap_or_else(|| default_provider().id().to_string());
        if let Some(index) = settings
            .provider_ids
            .iter()
            .position(|candidate| *candidate == provider_id)
        {
            settings.provider.selectItemAtIndex(index as isize);
        }
        settings.current_provider.replace(provider_id.clone());
        settings.provider.setEnabled(true);
        self.request_commit_message_models(provider_id, model);
    }

    fn apply_commit_message_models(
        &self,
        request_id: u64,
        provider_id: String,
        selected_model: Option<String>,
        result: Result<Vec<ModelOption>, String>,
    ) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        if settings.request_id.get() != request_id
            || settings.current_provider.borrow().as_str() != provider_id
        {
            return;
        }
        unsafe { settings.spinner.stopAnimation(None) };
        settings.spinner.setHidden(true);
        settings.model.removeAllItems();
        let provider = find_provider(&provider_id).unwrap_or_else(default_provider);
        settings
            .model
            .addItemWithTitle(&NSString::from_str(&provider.default_model_label()));
        let mut model_ids = vec![None];
        match result {
            Ok(models) => {
                for option in models {
                    settings
                        .model
                        .addItemWithTitle(&NSString::from_str(&option.label));
                    model_ids.push(Some(option.id));
                }
                let selected_index = selected_model
                    .as_ref()
                    .and_then(|selected| {
                        model_ids
                            .iter()
                            .position(|candidate| candidate.as_ref() == Some(selected))
                    })
                    .unwrap_or(0);
                settings.model.selectItemAtIndex(selected_index as isize);
                settings.model.setEnabled(true);
                settings.status.setStringValue(&NSString::from_str(
                    "Selections apply to the next generated commit message.",
                ));
            }
            Err(error) => {
                settings.model.selectItemAtIndex(0);
                settings.model.setEnabled(true);
                settings.status.setStringValue(&NSString::from_str(&error));
            }
        }
        settings.model_ids.replace(model_ids);
    }

    fn set_commit_message_settings_error(&self, message: &str) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        unsafe { settings.spinner.stopAnimation(None) };
        settings.spinner.setHidden(true);
        settings.provider.setEnabled(true);
        settings.model.setEnabled(false);
        settings.status.setStringValue(&NSString::from_str(message));
    }

    fn install_main_menu(&self, application: &NSApplication) {
        let mtm = self.mtm();
        let main = NSMenu::new(mtm);

        let app_root = NSMenuItem::new(mtm);
        app_root.setTitle(&NSString::from_str("Craic"));
        let app_menu = NSMenu::new(mtm);
        unsafe {
            app_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("About Craic"),
                Some(sel!(orderFrontStandardAboutPanel:)),
                &NSString::new(),
            );
        }
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let settings = unsafe {
            app_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Settings…"),
                Some(sel!(showSettings:)),
                &NSString::from_str(","),
            )
        };
        unsafe { settings.setTarget(Some(self)) };
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let services = NSMenuItem::new(mtm);
        services.setTitle(&NSString::from_str("Services"));
        let services_menu = NSMenu::new(mtm);
        services.setSubmenu(Some(&services_menu));
        application.setServicesMenu(Some(&services_menu));
        app_menu.addItem(&services);
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        unsafe {
            app_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Hide Craic"),
                Some(sel!(hide:)),
                &NSString::from_str("h"),
            );
        }
        let hide_others = unsafe {
            app_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Hide Others"),
                Some(sel!(hideOtherApplications:)),
                &NSString::from_str("h"),
            )
        };
        hide_others.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        );
        unsafe {
            app_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Show All"),
                Some(sel!(unhideAllApplications:)),
                &NSString::new(),
            );
        }
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        unsafe {
            app_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Quit Craic"),
                Some(sel!(terminate:)),
                &NSString::from_str("q"),
            );
        }
        app_root.setSubmenu(Some(&app_menu));
        main.addItem(&app_root);

        let file_root = NSMenuItem::new(mtm);
        file_root.setTitle(&NSString::from_str("File"));
        let file_menu = NSMenu::new(mtm);
        let new_window = unsafe {
            file_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("New Window"),
                Some(sel!(newWindow:)),
                &NSString::from_str("n"),
            )
        };
        unsafe { new_window.setTarget(Some(self)) };
        file_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let open_workspace = unsafe {
            file_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Open Workspace…"),
                Some(sel!(openWorkspace:)),
                &NSString::from_str("o"),
            )
        };
        unsafe { open_workspace.setTarget(Some(self)) };
        file_root.setSubmenu(Some(&file_menu));
        main.addItem(&file_root);

        let edit_root = NSMenuItem::new(mtm);
        edit_root.setTitle(&NSString::from_str("Edit"));
        let edit_menu = NSMenu::new(mtm);
        for (title, action, key) in [
            ("Undo", sel!(undo:), "z"),
            ("Redo", sel!(redo:), "Z"),
            ("Cut", sel!(cut:), "x"),
            ("Copy", sel!(copy:), "c"),
            ("Paste", sel!(paste:), "v"),
            ("Select All", sel!(selectAll:), "a"),
        ] {
            unsafe {
                edit_menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::from_str(key),
                );
            }
            if title == "Redo" || title == "Select All" {
                edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
        }
        let find = unsafe {
            edit_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Find…"),
                Some(sel!(findContent:)),
                &NSString::from_str("f"),
            )
        };
        find.setTag(NSFindPanelAction::ShowFindPanel.0 as isize);
        unsafe { find.setTarget(Some(self)) };
        edit_root.setSubmenu(Some(&edit_menu));
        main.addItem(&edit_root);

        let view_root = NSMenuItem::new(mtm);
        view_root.setTitle(&NSString::from_str("View"));
        let view_menu = NSMenu::new(mtm);
        for (index, descriptor) in PAGE_DESCRIPTORS.iter().enumerate() {
            let item = unsafe {
                view_menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(descriptor.label),
                    Some(sel!(activatePageFromMenu:)),
                    &NSString::from_str(&(index + 1).to_string()),
                )
            };
            item.setTag(index as isize);
            item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
            unsafe { item.setTarget(Some(self)) };
        }
        view_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let refresh = unsafe {
            view_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Refresh Workspace"),
                Some(sel!(refreshWorkspace:)),
                &NSString::from_str("r"),
            )
        };
        unsafe { refresh.setTarget(Some(self)) };
        let refresh_page = unsafe {
            view_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Refresh Current Page"),
                Some(sel!(refreshPage:)),
                &NSString::from_str("r"),
            )
        };
        refresh_page.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        );
        unsafe { refresh_page.setTarget(Some(self)) };
        view_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let increase_font = unsafe {
            view_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Increase Text Size"),
                Some(sel!(increaseFontSize:)),
                &NSString::from_str("="),
            )
        };
        increase_font.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        );
        unsafe { increase_font.setTarget(Some(self)) };
        let decrease_font = unsafe {
            view_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Decrease Text Size"),
                Some(sel!(decreaseFontSize:)),
                &NSString::from_str("-"),
            )
        };
        decrease_font.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        unsafe { decrease_font.setTarget(Some(self)) };
        let reset_font = unsafe {
            view_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Actual Text Size"),
                Some(sel!(resetFontSize:)),
                &NSString::from_str("0"),
            )
        };
        reset_font.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        unsafe { reset_font.setTarget(Some(self)) };
        view_root.setSubmenu(Some(&view_menu));
        main.addItem(&view_root);

        let source_control_root = NSMenuItem::new(mtm);
        source_control_root.setTitle(&NSString::from_str("Source Control"));
        let source_control_menu = NSMenu::new(mtm);
        let pull = unsafe {
            source_control_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Pull Remote Changes"),
                Some(sel!(pullRemote:)),
                &NSString::from_str("p"),
            )
        };
        unsafe { pull.setTarget(Some(self)) };
        let push = unsafe {
            source_control_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Push Local Commits"),
                Some(sel!(pushRemote:)),
                &NSString::from_str("u"),
            )
        };
        unsafe { push.setTarget(Some(self)) };
        source_control_root.setSubmenu(Some(&source_control_menu));
        main.addItem(&source_control_root);

        let help_root = NSMenuItem::new(mtm);
        help_root.setTitle(&NSString::from_str("Help"));
        let help_menu = NSMenu::new(mtm);
        let shortcuts = unsafe {
            help_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Keyboard Shortcuts"),
                Some(sel!(showKeyboardShortcuts:)),
                &NSString::from_str("?"),
            )
        };
        unsafe { shortcuts.setTarget(Some(self)) };
        help_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let website = unsafe {
            help_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Craic Website"),
                Some(sel!(openCraicWebsite:)),
                &NSString::new(),
            )
        };
        unsafe { website.setTarget(Some(self)) };
        let issues = unsafe {
            help_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Report an Issue"),
                Some(sel!(reportCraicIssue:)),
                &NSString::new(),
            )
        };
        unsafe { issues.setTarget(Some(self)) };
        help_root.setSubmenu(Some(&help_menu));
        main.addItem(&help_root);

        application.setMainMenu(Some(&main));
    }

    fn toolbar_item_identifiers(&self) -> Retained<NSArray<NSToolbarItemIdentifier>> {
        let workspace = NSString::from_str(TOOLBAR_WORKSPACE);
        let pages = NSString::from_str(TOOLBAR_PAGES);
        let branch = NSString::from_str(TOOLBAR_BRANCH);
        let fetch = NSString::from_str(TOOLBAR_FETCH);
        let terminal = NSString::from_str(TOOLBAR_TERMINAL);
        let add_action = NSString::from_str(TOOLBAR_ADD_ACTION);
        NSArray::from_slice(&[
            &pages,
            &workspace,
            &branch,
            unsafe { NSToolbarFlexibleSpaceItemIdentifier },
            &add_action,
            unsafe { NSToolbarSpaceItemIdentifier },
            &terminal,
            &fetch,
        ])
    }

}
