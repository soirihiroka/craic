impl AppDelegate {
    fn workspace_create_name_did_change(&self, sender: &NSTextField) {
        if !self.ivars().workspace_create_updating_name.get() {
            self.ivars()
                .workspace_create_auto_name
                .set(sender.stringValue().to_string().trim().is_empty());
        }
        self.update_workspace_create_button();
    }

    fn workspace_create_remote_did_change(&self, sender: &NSTextField) {
        let Some(name) = self.ivars().workspace_create_name.borrow().clone() else {
            return;
        };
        if self.ivars().workspace_create_auto_name.get()
            || name.stringValue().to_string().trim().is_empty()
        {
            let derived = native_workspace_name_from_remote(&sender.stringValue().to_string())
                .unwrap_or_default();
            self.ivars().workspace_create_updating_name.set(true);
            name.setStringValue(&NSString::from_str(&derived));
            self.ivars().workspace_create_updating_name.set(false);
            self.ivars().workspace_create_auto_name.set(true);
        }
        self.update_workspace_create_button();
    }

    fn update_workspace_create_button(&self) {
        let Some(button) = self.ivars().workspace_create_button.borrow().clone() else {
            return;
        };
        let has_explicit_name = self
            .ivars()
            .workspace_create_name
            .borrow()
            .as_ref()
            .is_some_and(|field| !field.stringValue().to_string().trim().is_empty());
        let has_derived_name = self
            .ivars()
            .workspace_create_remote
            .borrow()
            .as_ref()
            .and_then(|field| native_workspace_name_from_remote(&field.stringValue().to_string()))
            .is_some();
        button.setEnabled(
            !self.ivars().workspace_create_in_progress.get()
                && self.ivars().workspace_create_has_root.get()
                && (has_explicit_name || has_derived_name),
        );
    }

    fn show_create_workspace_dialog(&self) {
        if self.ivars().workspace_create_form.borrow().is_some() {
            return;
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let roots = craic_config::load()
            .workspace_roots
            .into_iter()
            .filter_map(|root| {
                root.provider.is_local().then(|| {
                    craic_config::expand_config_path_for_ui(&root.path)
                        .map(|path| path.canonicalize().unwrap_or(path))
                })?
            })
            .collect::<Vec<_>>();

        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Create Workspace"));
        alert.setInformativeText(&NSString::from_str(
            "Create an empty workspace or clone a Git repository into a configured local root.",
        ));
        let create = alert.addButtonWithTitle(&NSString::from_str("Create"));
        let cancel = alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        unsafe {
            create.setTarget(Some(self));
            create.setAction(Some(sel!(submitWorkspaceCreation:)));
        }

        let accessory = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(440.0, 172.0)),
        );
        let root_label =
            NSTextField::labelWithString(&NSString::from_str("Workspace Root"), self.mtm());
        root_label.setFrame(NSRect::new(
            NSPoint::new(0.0, 148.0),
            NSSize::new(130.0, 18.0),
        ));
        accessory.addSubview(&root_label);
        let root = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm()),
            NSRect::new(NSPoint::new(138.0, 142.0), NSSize::new(302.0, 28.0)),
            false,
        );
        if roots.is_empty() {
            root.addItemWithTitle(&NSString::from_str("No configured local roots"));
            root.setEnabled(false);
        } else {
            for path in &roots {
                root.addItemWithTitle(&NSString::from_str(&path.display().to_string()));
            }
        }
        root.setToolTip(Some(&NSString::from_str("Configured local workspace root")));
        accessory.addSubview(&root);

        let name_label =
            NSTextField::labelWithString(&NSString::from_str("Repository Name"), self.mtm());
        name_label.setFrame(NSRect::new(
            NSPoint::new(0.0, 106.0),
            NSSize::new(130.0, 18.0),
        ));
        accessory.addSubview(&name_label);
        let name = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::new(138.0, 100.0), NSSize::new(302.0, 26.0)),
        );
        name.setPlaceholderString(Some(&NSString::from_str("Repository name")));
        unsafe {
            name.setTarget(Some(self));
            name.setAction(Some(sel!(workspaceCreateNameChanged:)));
            name.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        accessory.addSubview(&name);

        let remote_label =
            NSTextField::labelWithString(&NSString::from_str("Remote Git Source"), self.mtm());
        remote_label.setFrame(NSRect::new(
            NSPoint::new(0.0, 64.0),
            NSSize::new(130.0, 18.0),
        ));
        accessory.addSubview(&remote_label);
        let remote = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::new(138.0, 58.0), NSSize::new(302.0, 26.0)),
        );
        remote.setPlaceholderString(Some(&NSString::from_str("Optional Git URL")));
        remote.setToolTip(Some(&NSString::from_str(
            "Optional remote repository to clone",
        )));
        unsafe {
            remote.setTarget(Some(self));
            remote.setAction(Some(sel!(workspaceCreateRemoteChanged:)));
            remote.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        accessory.addSubview(&remote);
        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(self.mtm()),
            NSRect::new(NSPoint::new(138.0, 12.0), NSSize::new(16.0, 16.0)),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setIndeterminate(true);
        spinner.setDisplayedWhenStopped(false);
        spinner.setHidden(true);
        accessory.addSubview(&spinner);
        let status = NSTextField::labelWithString(&NSString::new(), self.mtm());
        status.setFrame(NSRect::new(
            NSPoint::new(162.0, 10.0),
            NSSize::new(278.0, 20.0),
        ));
        status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        accessory.addSubview(&status);
        alert.setAccessoryView(Some(&accessory));

        self.ivars().workspace_create_auto_name.set(true);
        self.ivars().workspace_create_updating_name.set(false);
        self.ivars()
            .workspace_create_has_root
            .set(!roots.is_empty());
        self.ivars()
            .workspace_create_name
            .replace(Some(name.clone()));
        self.ivars()
            .workspace_create_remote
            .replace(Some(remote.clone()));
        self.ivars().workspace_create_root.replace(Some(root));
        self.ivars().workspace_create_roots.replace(roots);
        self.ivars()
            .workspace_create_button
            .replace(Some(create.clone()));
        self.ivars()
            .workspace_create_cancel_button
            .replace(Some(cancel));
        self.ivars().workspace_create_spinner.replace(Some(spinner));
        self.ivars().workspace_create_status.replace(Some(status));
        self.ivars()
            .workspace_create_pending_success
            .borrow_mut()
            .take();
        self.ivars()
            .workspace_create_form
            .replace(Some(alert.clone()));
        create.setEnabled(false);
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            let success = delegate
                .ivars()
                .workspace_create_pending_success
                .borrow_mut()
                .take();
            delegate.ivars().workspace_create_in_progress.set(false);
            if let Some(add) = delegate.ivars().workspace_add_button.get() {
                add.setEnabled(true);
            }
            delegate.ivars().workspace_create_name.borrow_mut().take();
            delegate.ivars().workspace_create_remote.borrow_mut().take();
            delegate.ivars().workspace_create_root.borrow_mut().take();
            delegate.ivars().workspace_create_roots.borrow_mut().clear();
            delegate.ivars().workspace_create_button.borrow_mut().take();
            delegate
                .ivars()
                .workspace_create_cancel_button
                .borrow_mut()
                .take();
            delegate
                .ivars()
                .workspace_create_spinner
                .borrow_mut()
                .take();
            delegate.ivars().workspace_create_status.borrow_mut().take();
            delegate.ivars().workspace_create_form.borrow_mut().take();
            if response == NSAlertFirstButtonReturn
                && let Some((path, message)) = success
            {
                delegate.apply_workspace_creation_success(path, message);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        window.makeFirstResponder(Some(&name));
    }

    fn submit_workspace_creation(&self) {
        if self.ivars().workspace_create_in_progress.get() {
            return;
        }
        let (Some(root), Some(name), Some(remote)) = (
            self.ivars().workspace_create_root.borrow().clone(),
            self.ivars().workspace_create_name.borrow().clone(),
            self.ivars().workspace_create_remote.borrow().clone(),
        ) else {
            return;
        };
        let Ok(root_index) = usize::try_from(root.indexOfSelectedItem()) else {
            self.set_workspace_create_status("Choose a workspace root.", true);
            return;
        };
        let Some(root) = self
            .ivars()
            .workspace_create_roots
            .borrow()
            .get(root_index)
            .cloned()
        else {
            self.set_workspace_create_status("Choose a configured local workspace root.", true);
            return;
        };
        match native_create_workspace_request(
            root,
            &name.stringValue().to_string(),
            &remote.stringValue().to_string(),
        ) {
            Ok(request) => self.start_workspace_creation(request),
            Err(error) => self.set_workspace_create_status(&error, true),
        }
    }

    fn set_workspace_create_status(&self, message: &str, is_error: bool) {
        let Some(status) = self.ivars().workspace_create_status.borrow().clone() else {
            return;
        };
        status.setStringValue(&NSString::from_str(message));
        let color = if is_error {
            NSColor::systemRedColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        status.setTextColor(Some(&color));
        status.setToolTip(
            (!message.is_empty())
                .then(|| NSString::from_str(message))
                .as_deref(),
        );
    }

    fn start_workspace_creation(&self, request: NativeCreateWorkspaceRequest) {
        if self.ivars().workspace_create_in_progress.replace(true) {
            return;
        }
        let request_id = self
            .ivars()
            .workspace_create_request_id
            .get()
            .wrapping_add(1);
        self.ivars().workspace_create_request_id.set(request_id);
        if let Some(add) = self.ivars().workspace_add_button.get() {
            add.setEnabled(false);
        }
        if let Some(root) = self.ivars().workspace_create_root.borrow().as_ref() {
            root.setEnabled(false);
        }
        if let Some(name) = self.ivars().workspace_create_name.borrow().as_ref() {
            name.setEnabled(false);
        }
        if let Some(remote) = self.ivars().workspace_create_remote.borrow().as_ref() {
            remote.setEnabled(false);
        }
        if let Some(create) = self.ivars().workspace_create_button.borrow().as_ref() {
            create.setTitle(&NSString::from_str("Creating…"));
            create.setEnabled(false);
        }
        if let Some(cancel) = self
            .ivars()
            .workspace_create_cancel_button
            .borrow()
            .as_ref()
        {
            cancel.setEnabled(false);
        }
        if let Some(spinner) = self.ivars().workspace_create_spinner.borrow().as_ref() {
            spinner.setHidden(false);
            unsafe { spinner.startAnimation(None) };
        }
        self.set_workspace_create_status("Creating workspace…", false);
        let Some(requests) = self.ivars().frontend_requests.get() else {
            self.finish_workspace_creation(
                request_id,
                Err("The workspace creation service is unavailable.".to_string()),
            );
            return;
        };
        if let Err(error) = requests.try_send(FrontendRequest::CreateWorkspace {
            request_id,
            request,
        }) {
            self.finish_workspace_creation(
                request_id,
                Err(format!("Workspace creation could not be queued: {error}")),
            );
        }
    }

    fn finish_workspace_creation(
        &self,
        request_id: u64,
        result: Result<(PathBuf, String), String>,
    ) {
        if self.ivars().workspace_create_request_id.get() != request_id
            || !self.ivars().workspace_create_in_progress.replace(false)
        {
            return;
        }
        let (path, message) = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(add) = self.ivars().workspace_add_button.get() {
                    add.setEnabled(true);
                }
                if let Some(root) = self.ivars().workspace_create_root.borrow().as_ref() {
                    root.setEnabled(true);
                }
                if let Some(name) = self.ivars().workspace_create_name.borrow().as_ref() {
                    name.setEnabled(true);
                }
                if let Some(remote) = self.ivars().workspace_create_remote.borrow().as_ref() {
                    remote.setEnabled(true);
                }
                if let Some(cancel) = self
                    .ivars()
                    .workspace_create_cancel_button
                    .borrow()
                    .as_ref()
                {
                    cancel.setEnabled(true);
                }
                if let Some(create) = self.ivars().workspace_create_button.borrow().as_ref() {
                    create.setTitle(&NSString::from_str("Create"));
                }
                if let Some(spinner) = self.ivars().workspace_create_spinner.borrow().as_ref() {
                    unsafe { spinner.stopAnimation(None) };
                    spinner.setHidden(true);
                }
                self.set_workspace_create_status(&error, true);
                self.update_workspace_create_button();
                return;
            }
        };
        self.ivars()
            .workspace_create_pending_success
            .replace(Some((path, message)));
        let Some(alert) = self.ivars().workspace_create_form.borrow().clone() else {
            return;
        };
        if let Some(window) = self.ivars().window.get() {
            window.endSheet_returnCode(&alert.window(), NSAlertFirstButtonReturn);
        }
    }

    fn apply_workspace_creation_success(&self, path: PathBuf, message: String) {
        let workspace = craic_config::ConfiguredWorkspace::local(path.to_string_lossy());
        let entry = WorkspaceEntry {
            label: workspace.label(),
            workspace,
        };
        if !self
            .ivars()
            .workspaces
            .borrow()
            .iter()
            .any(|candidate| candidate.selection_id() == entry.selection_id())
        {
            self.ivars().workspaces.borrow_mut().push(entry.clone());
            self.ivars()
                .workspaces
                .borrow_mut()
                .sort_by_key(|candidate| candidate.label.to_lowercase());
            self.queue_workspace_metadata(vec![entry.clone()]);
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
                    .find(|candidate| candidate.selection_id() == workspace_id)
                    .map(|candidate| candidate.workspace.clone())
            });
        let filter = self
            .ivars()
            .workspace_search
            .get()
            .map(|search| search.stringValue().to_string())
            .unwrap_or_default();
        self.refresh_workspace_results(&filter);
        self.prompt_workspace_open(entry, Some(message));
        self.request_workspace_discovery(preferred, false);
    }

}
