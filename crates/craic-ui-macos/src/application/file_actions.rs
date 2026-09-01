impl AppDelegate {
    fn selected_workspace_file_info(&self) -> Option<(Arc<dyn FileAccess>, FileNodeInfo)> {
        let files = self.ivars().files.get()?;
        let path = files.selected_path.borrow().clone()?;
        let info = files
            .rows
            .borrow()
            .iter()
            .find(|row| row.info.path == path)
            .map(|row| row.info.clone())?;
        let access = self
            .ivars()
            .workspace_handle
            .borrow()
            .as_ref()?
            .workspace_files();
        Some((access, info))
    }

    fn selected_workspace_file(&self) -> Option<(Arc<dyn FileAccess>, FileNodePath, FileNodeKind)> {
        let (access, info) = self.selected_workspace_file_info()?;
        Some((access, info.path, info.kind))
    }

    fn open_selected_workspace_entry(&self) {
        let Some((_access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.path.is_root() {
            self.open_selected_workspace_file_external();
            return;
        }
        let Some(row) = self
            .filtered_file_tree_rows()
            .iter()
            .position(|row| row.info.path == info.path)
        else {
            return;
        };
        if info.kind.is_directory() || info.capabilities.listable {
            self.toggle_file_tree_row(row);
        } else {
            self.select_file_tree_row(row);
        }
    }

    fn open_selected_workspace_file_external(&self) {
        let Some((access, path, _kind)) = self.selected_workspace_file() else {
            return;
        };
        let Some(local_path) = access.local_path(&path) else {
            self.present_path_action_error(
                "Unable to Open Item",
                "Opening this provider item with a macOS application is unavailable.",
            );
            return;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(&local_path.to_string_lossy()));
        if !NSWorkspace::sharedWorkspace().openURL(&url) {
            self.present_path_action_error(
                "Unable to Open Item",
                &format!("No application could open {}.", local_path.display()),
            );
        }
    }

    fn paste_workspace_file_from_clipboard(&self) {
        let Some((access, destination_parent)) = self.workspace_file_creation_parent() else {
            return;
        };
        let Some((source, move_item)) = workspace_file_clipboard_from_pasteboard(
            &NSPasteboard::generalPasteboard(),
            access.as_ref(),
        ) else {
            return;
        };
        if source == destination_parent
            || destination_parent.is_descendant_of(&source)
        {
            self.present_path_action_error(
                "Paste Failed",
                "An item cannot be pasted into itself or one of its descendants.",
            );
            return;
        }
        let Some(name) = source.file_name().map(ToString::to_string) else {
            return;
        };
        let mutation = if move_item {
            NativeFileMutation::Move {
                source,
                destination_parent,
                new_name: name,
            }
        } else {
            NativeFileMutation::Copy {
                source,
                destination: destination_parent.join_child(name),
            }
        };
        self.request_file_mutation(access, mutation);
    }

    fn add_workspace_file_menu_item(
        &self,
        menu: &NSMenu,
        title: &str,
        action: Sel,
        key: &str,
        enabled: bool,
        represented: Option<&str>,
    ) -> Retained<NSMenuItem> {
        let item = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        unsafe {
            item.setTarget(Some(self));
            if let Some(represented) = represented {
                item.setRepresentedObject(Some(&NSString::from_str(represented)));
            }
        }
        item.setEnabled(enabled);
        item
    }

    fn rebuild_workspace_file_menu(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let menu = &files.menu;
        menu.removeAllItems();
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        let is_root = info.path.is_root();
        let is_directory = info.kind.is_directory();
        let is_native = access.local_path(&access.root()).is_some();
        let idle = !files.mutation_in_progress.get();

        if is_directory {
            self.add_workspace_file_menu_item(
                menu,
                "New File…",
                sel!(newWorkspaceFile:),
                "",
                idle && info.capabilities.creatable,
                None,
            );
            self.add_workspace_file_menu_item(
                menu,
                "New Folder…",
                sel!(newWorkspaceFolder:),
                "",
                idle && info.capabilities.creatable,
                None,
            );
            self.add_workspace_file_menu_item(
                menu,
                "Upload…",
                sel!(uploadWorkspaceFiles:),
                "",
                idle && info.capabilities.creatable,
                None,
            );
            menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        }

        self.add_workspace_file_menu_item(
            menu,
            "Open",
            sel!(activateWorkspaceSelection:),
            "",
            !is_directory || info.capabilities.listable || (is_root && is_native),
            None,
        );
        self.add_workspace_file_menu_item(
            menu,
            "Open With…",
            sel!(openWorkspaceFile:),
            "",
            info.capabilities.open_external,
            None,
        );
        self.add_workspace_file_menu_item(
            menu,
            "Reveal in Finder",
            sel!(revealWorkspaceFile:),
            "",
            info.capabilities.reveal,
            None,
        );
        self.add_workspace_file_menu_item(
            menu,
            "Open in Integrated Terminal",
            sel!(openWorkspaceFileInTerminal:),
            "",
            self.ivars().workspace_handle.borrow().is_some(),
            None,
        );
        if !is_directory && info.executable() && is_native {
            self.add_workspace_file_menu_item(
                menu,
                "Run in Integrated Terminal",
                sel!(runWorkspaceFileInTerminal:),
                "",
                true,
                None,
            );
        }
        if access.supports_download() && !is_root && info.capabilities.readable {
            self.add_workspace_file_menu_item(
                menu,
                "Download…",
                sel!(downloadWorkspaceFile:),
                "",
                idle,
                None,
            );
        }
        if !is_directory {
            self.add_workspace_file_menu_item(
                menu,
                "Add File to Chat",
                sel!(addWorkspaceFileToChat:),
                "",
                info.capabilities.readable,
                None,
            );
        }

        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        if !is_root {
            self.add_workspace_file_menu_item(
                menu,
                "Cut",
                sel!(cutWorkspaceFile:),
                "x",
                idle && info.capabilities.movable && is_native,
                None,
            );
            self.add_workspace_file_menu_item(
                menu,
                "Copy",
                sel!(copyWorkspaceFile:),
                "c",
                idle && info.capabilities.readable,
                None,
            );
        }
        let can_paste = self
            .workspace_file_creation_parent()
            .and_then(|(destination_access, destination_parent)| {
                workspace_file_clipboard_from_pasteboard(
                    &NSPasteboard::generalPasteboard(),
                    destination_access.as_ref(),
                )
                .map(|(source, _)| (source, destination_parent))
            })
            .is_some_and(|(source, destination_parent)| {
                source != destination_parent
                    && !destination_parent.is_descendant_of(&source)
            });
        self.add_workspace_file_menu_item(
            menu,
            "Paste",
            sel!(pasteWorkspaceFile:),
            "v",
            idle && can_paste,
            None,
        );

        if !is_root && is_native {
            let kind = if is_directory {
                IgnoreTargetKind::Folder
            } else {
                IgnoreTargetKind::File
            };
            let options = gitignore::options_for_path(&info.path.display(), kind);
            if options.direct.is_some()
                || !options.folders.is_empty()
                || options.extension.is_some()
            {
                menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            }
            if let Some(option) = options.direct {
                self.add_workspace_file_menu_item(
                    menu,
                    &option.label,
                    sel!(addWorkspaceIgnorePattern:),
                    "",
                    true,
                    Some(&option.pattern),
                );
            }
            if !options.folders.is_empty() {
                let folders = NSMenu::new(self.mtm());
                folders.setAutoenablesItems(false);
                for option in options.folders {
                    self.add_workspace_file_menu_item(
                        &folders,
                        &option.label,
                        sel!(addWorkspaceIgnorePattern:),
                        "",
                        true,
                        Some(&option.pattern),
                    );
                }
                let item = NSMenuItem::new(self.mtm());
                item.setTitle(&NSString::from_str("Ignore Folder (Add to .gitignore)"));
                item.setSubmenu(Some(&folders));
                menu.addItem(&item);
            }
            if let Some(option) = options.extension {
                self.add_workspace_file_menu_item(
                    menu,
                    &option.label,
                    sel!(addWorkspaceIgnorePattern:),
                    "",
                    true,
                    Some(&option.pattern),
                );
            }
        }

        if !is_directory && is_native {
            let display_path = info.path.display();
            let support = resolve_file_support(FileProbe {
                path: &display_path,
                is_dir: false,
                leading_bytes: None,
            });
            let actions: &[(&str, &str)] = match support.role {
                Some(FileRole::Dockerfile) => &[("Build Image", "build-image")],
                Some(FileRole::Compose) => &[
                    ("Compose Logs", "compose-logs"),
                    ("Compose Up", "compose-up"),
                    ("Compose Pull", "compose-pull"),
                    ("Compose Restart", "compose-restart"),
                    ("Compose Down", "compose-down"),
                ],
                None => &[],
            };
            if !actions.is_empty() {
                menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
                for (title, action) in actions {
                    self.add_workspace_file_menu_item(
                        menu,
                        title,
                        sel!(runWorkspaceContainerFileAction:),
                        "",
                        true,
                        Some(action),
                    );
                }
            }
        }

        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        self.add_workspace_file_menu_item(
            menu,
            "Copy Workspace Path",
            sel!(copyWorkspaceFileProviderPath:),
            "",
            true,
            None,
        );
        if !is_root {
            self.add_workspace_file_menu_item(
                menu,
                "Copy Relative Path",
                sel!(copyWorkspaceFileRelativePath:),
                "",
                true,
                None,
            );
        }

        if !is_root {
            menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            let can_duplicate = info.capabilities.readable
                && info.path.parent().is_some_and(|parent| {
                    files
                        .rows
                        .borrow()
                        .iter()
                        .any(|row| row.info.path == parent && row.info.capabilities.creatable)
                });
            self.add_workspace_file_menu_item(
                menu,
                "Duplicate…",
                sel!(duplicateWorkspaceFile:),
                "",
                idle && can_duplicate,
                None,
            );
            self.add_workspace_file_menu_item(
                menu,
                "Move…",
                sel!(moveWorkspaceFile:),
                "",
                idle && info.capabilities.movable,
                None,
            );
            self.add_workspace_file_menu_item(
                menu,
                "Rename…",
                sel!(renameWorkspaceFile:),
                "",
                idle && info.capabilities.movable,
                None,
            );
            self.add_workspace_file_menu_item(
                menu,
                "Delete…",
                sel!(deleteWorkspaceFile:),
                "",
                idle && info.capabilities.deletable,
                None,
            );
        }
    }

    fn workspace_file_creation_parent(&self) -> Option<(Arc<dyn FileAccess>, FileNodePath)> {
        let (access, info) = self.selected_workspace_file_info()?;
        if info.kind.is_directory() && info.capabilities.creatable {
            return Some((access, info.path));
        }
        let parent = info.path.parent()?;
        let files = self.ivars().files.get()?;
        let creatable = files
            .rows
            .borrow()
            .iter()
            .find(|row| row.info.path == parent)
            .is_some_and(|row| row.info.capabilities.creatable);
        creatable.then_some((access, parent))
    }

    fn workspace_file_drop_parent(
        &self,
        row: isize,
    ) -> Option<(Arc<dyn FileAccess>, FileNodePath)> {
        let row = usize::try_from(row).ok()?;
        let row = self.filtered_file_tree_rows().get(row).cloned()?;
        if !row.info.kind.is_directory() || !row.info.capabilities.creatable {
            return None;
        }
        let access = self
            .ivars()
            .workspace_handle
            .borrow()
            .as_ref()?
            .workspace_files();
        Some((access, row.info.path))
    }

    fn schedule_file_drop_auto_expand(&self, path: &FileNodePath) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.expanded.borrow().contains(path)
            || files.drop_hover_path.borrow().as_ref() == Some(path)
        {
            return;
        }
        let generation = files.drop_hover_generation.get().wrapping_add(1);
        files.drop_hover_generation.set(generation);
        files.drop_hover_path.replace(Some(path.clone()));
        let hover_path = path.clone();
        let delegate = MainThreadBound::new(self.retain(), self.mtm());
        let when = DispatchTime::try_from(Duration::from_millis(500))
            .expect("500 milliseconds fits dispatch time");
        let _ = DispatchQueue::main().after(when, move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let delegate = delegate.get(mtm);
            let Some(files) = delegate.ivars().files.get() else {
                return;
            };
            if files.drop_hover_generation.get() != generation
                || files.drop_hover_path.borrow().as_ref() != Some(&hover_path)
                || files.expanded.borrow().contains(&hover_path)
            {
                return;
            }
            files.expanded.borrow_mut().insert(hover_path);
            delegate.request_files_tree();
        });
    }

    fn clear_file_drop_hover(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files
            .drop_hover_generation
            .set(files.drop_hover_generation.get().wrapping_add(1));
        files.drop_hover_path.borrow_mut().take();
    }

    fn prompt_new_workspace_entry(&self, directory: bool) {
        let Some((access, parent)) = self.workspace_file_creation_parent() else {
            return;
        };
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let label = if directory { "Folder" } else { "File" };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(&format!("New {label}")));
        alert.setInformativeText(&NSString::from_str(&format!(
            "Create a new {} in {}.",
            label.to_lowercase(),
            if parent.is_root() {
                "the workspace root".to_string()
            } else {
                parent.display()
            }
        )));
        alert.addButtonWithTitle(&NSString::from_str("Create"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(360.0, 24.0)),
        );
        input.setPlaceholderString(Some(&NSString::from_str(&format!("{label} name"))));
        alert.setAccessoryView(Some(&input));
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let name = completion_input
                .stringValue()
                .to_string()
                .trim()
                .to_string();
            if name.is_empty() {
                delegate.present_path_action_error(
                    &format!("New {label} Failed"),
                    &format!("Enter a {label} name."),
                );
                return;
            }
            let path = parent.join_child(&name);
            delegate.request_file_mutation(
                access.clone(),
                if directory {
                    NativeFileMutation::CreateDirectory { path }
                } else {
                    NativeFileMutation::CreateFile { path }
                },
            );
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        window.makeFirstResponder(Some(&input));
    }

    fn prompt_rename_workspace_file(&self) {
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.path.is_root() || !info.capabilities.movable {
            return;
        }
        let Some(parent) = info.path.parent() else {
            return;
        };
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let current_name = info
            .path
            .file_name()
            .unwrap_or(info.display_name.as_str())
            .to_string();
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Rename Item"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "Enter a new name for {current_name}."
        )));
        alert.addButtonWithTitle(&NSString::from_str("Rename"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(360.0, 24.0)),
        );
        input.setStringValue(&NSString::from_str(&current_name));
        alert.setAccessoryView(Some(&input));
        let source = info.path;
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let new_name = completion_input
                .stringValue()
                .to_string()
                .trim()
                .to_string();
            if new_name.is_empty() {
                delegate.present_path_action_error("Rename Failed", "Enter a new item name.");
                return;
            }
            if new_name == current_name {
                return;
            }
            delegate.request_file_mutation(
                access.clone(),
                NativeFileMutation::Rename {
                    source: source.clone(),
                    destination_parent: parent.clone(),
                    new_name,
                },
            );
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        window.makeFirstResponder(Some(&input));
    }

    fn prompt_duplicate_workspace_file(&self) {
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.path.is_root() || !info.capabilities.readable {
            return;
        }
        let Some(parent) = info.path.parent() else {
            return;
        };
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let current_name = info
            .path
            .file_name()
            .unwrap_or(info.display_name.as_str())
            .to_string();
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Duplicate Item"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "Enter a name for the copy of {current_name}."
        )));
        alert.addButtonWithTitle(&NSString::from_str("Duplicate"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(360.0, 24.0)),
        );
        input.setStringValue(&NSString::from_str(&duplicate_file_name(
            &current_name,
            info.kind.is_directory(),
        )));
        alert.setAccessoryView(Some(&input));
        let source = info.path;
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let name = completion_input
                .stringValue()
                .to_string()
                .trim()
                .to_string();
            if name.is_empty() {
                delegate.present_path_action_error("Duplicate Failed", "Enter a copy name.");
                return;
            }
            delegate.request_file_mutation(
                access.clone(),
                NativeFileMutation::Copy {
                    source: source.clone(),
                    destination: parent.join_child(name),
                },
            );
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        window.makeFirstResponder(Some(&input));
    }

    fn prompt_move_workspace_file(&self) {
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.path.is_root() || !info.capabilities.movable {
            return;
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let source = info.path;
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Move Item"));
        alert.setInformativeText(&NSString::from_str(
            "Enter the destination path relative to the workspace root, including the item name.",
        ));
        alert.addButtonWithTitle(&NSString::from_str("Move"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let input = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(360.0, 24.0)),
        );
        input.setStringValue(&NSString::from_str(&source.display()));
        alert.setAccessoryView(Some(&input));
        let delegate = self.retain();
        let completion_input = input.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let relative = completion_input
                .stringValue()
                .to_string()
                .trim()
                .trim_start_matches('/')
                .to_string();
            if relative.is_empty() {
                delegate.present_path_action_error(
                    "Move Failed",
                    "Enter a destination inside the workspace.",
                );
                return;
            }
            let destination = access.root().join_child(&relative);
            if destination == source {
                return;
            }
            let Some(destination_parent) = destination.parent() else {
                delegate.present_path_action_error(
                    "Move Failed",
                    "The workspace root cannot be replaced.",
                );
                return;
            };
            let Some(new_name) = destination.file_name().map(ToString::to_string) else {
                delegate.present_path_action_error("Move Failed", "Enter a valid item name.");
                return;
            };
            delegate.request_file_mutation(
                access.clone(),
                NativeFileMutation::Move {
                    source: source.clone(),
                    destination_parent,
                    new_name,
                },
            );
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        window.makeFirstResponder(Some(&input));
    }

    fn confirm_delete_workspace_file(&self) {
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.path.is_root() || !info.capabilities.deletable {
            return;
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let path = info.path;
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Delete Item?"));
        let message = if info.kind.is_directory() {
            format!(
                "Delete the folder “{}” and everything inside it? This cannot be undone.",
                info.display_name
            )
        } else {
            format!(
                "Delete the file “{}”? This cannot be undone.",
                info.display_name
            )
        };
        alert.setInformativeText(&NSString::from_str(&message));
        alert.addButtonWithTitle(&NSString::from_str("Delete"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        alert.setAlertStyle(NSAlertStyle::Warning);
        if let Some(button) = alert.buttons().firstObject() {
            button.setHasDestructiveAction(true);
        }
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response == NSAlertFirstButtonReturn {
                delegate.request_file_mutation(
                    access.clone(),
                    NativeFileMutation::Delete { path: path.clone() },
                );
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn choose_workspace_file_download_destination(&self) {
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if !access.supports_download()
            || info.path.is_root()
            || !info.capabilities.readable
            || self
                .ivars()
                .files
                .get()
                .is_some_and(|files| files.mutation_in_progress.get())
        {
            return;
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let source = info.path;
        let delegate = self.retain();
        if info.kind.is_directory() {
            let panel = NSOpenPanel::openPanel(self.mtm());
            panel.setTitle(Some(&NSString::from_str("Choose Download Folder")));
            panel.setPrompt(Some(&NSString::from_str("Download")));
            panel.setCanChooseFiles(false);
            panel.setCanChooseDirectories(true);
            panel.setAllowsMultipleSelection(false);
            let retained_panel = panel.clone();
            let completion = RcBlock::new(move |response| {
                if response != NSModalResponseOK {
                    return;
                }
                let Some(destination) = retained_panel
                    .URLs()
                    .firstObject()
                    .and_then(|url| url.path())
                    .map(|path| PathBuf::from(path.to_string()))
                else {
                    delegate.present_path_action_error(
                        "Download Failed",
                        "Choose a destination on the local filesystem.",
                    );
                    return;
                };
                delegate.request_workspace_file_download(
                    access.clone(),
                    source.clone(),
                    FileDownloadDestination::Folder(destination),
                );
            });
            panel.beginSheetModalForWindow_completionHandler(window, &completion);
            return;
        }

        let panel = NSSavePanel::savePanel(self.mtm());
        panel.setTitle(Some(&NSString::from_str("Download File")));
        panel.setPrompt(Some(&NSString::from_str("Download")));
        if let Some(name) = source.file_name() {
            panel.setNameFieldStringValue(&NSString::from_str(name));
        }
        let retained_panel = panel.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSModalResponseOK {
                return;
            }
            let Some(destination) = retained_panel
                .URL()
                .and_then(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
            else {
                delegate.present_path_action_error(
                    "Download Failed",
                    "Choose a destination on the local filesystem.",
                );
                return;
            };
            delegate.request_workspace_file_download(
                access.clone(),
                source.clone(),
                FileDownloadDestination::File(destination),
            );
        });
        panel.beginSheetModalForWindow_completionHandler(window, &completion);
    }

    fn choose_workspace_files_to_upload(&self) {
        let Some((access, destination_parent)) = self.workspace_file_creation_parent() else {
            return;
        };
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setTitle(Some(&NSString::from_str("Choose Files to Upload")));
        panel.setPrompt(Some(&NSString::from_str("Upload")));
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(true);
        let retained_panel = panel.clone();
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response != NSModalResponseOK {
                return;
            }
            let sources = retained_panel
                .URLs()
                .iter()
                .filter_map(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
                .collect::<Vec<_>>();
            if sources.is_empty() {
                delegate.present_path_action_error(
                    "Upload Failed",
                    "Choose at least one file or folder.",
                );
                return;
            }
            delegate.request_file_mutation(
                access.clone(),
                NativeFileMutation::Upload {
                    sources,
                    destination_parent: destination_parent.clone(),
                },
            );
        });
        panel.beginSheetModalForWindow_completionHandler(window, &completion);
    }

    fn request_workspace_file_download(
        &self,
        access: Arc<dyn FileAccess>,
        source: FileNodePath,
        destination: FileDownloadDestination,
    ) {
        self.request_workspace_file_download_with_retry(access, source, destination, true);
    }

    fn request_workspace_file_download_with_retry(
        &self,
        access: Arc<dyn FileAccess>,
        source: FileNodePath,
        destination: FileDownloadDestination,
        allow_sudo_retry: bool,
    ) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.mutation_in_progress.replace(true) {
            return;
        }
        files.status.setHidden(false);
        files
            .status
            .setStringValue(&NSString::from_str("Downloading item…"));
        files.spinner.setHidden(false);
        unsafe { files.spinner.startAnimation(None) };
        self.set_page_badge("files", NativePageBadge::Indicator);
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.finish_workspace_file_download(
                &workspace_id,
                access.clone(),
                source.clone(),
                destination.clone(),
                allow_sudo_retry,
                Err("The repository service is unavailable.".to_string()),
            );
            return;
        };
        let completion_access = access.clone();
        let completion_source = source.clone();
        let completion_destination = destination.clone();
        if let Err(error) = requests.try_send(RepositoryRequest::DownloadWorkspaceFile {
            workspace_id: workspace_id.clone(),
            access,
            source,
            destination,
            allow_sudo_retry,
            cancellation,
        }) {
            self.finish_workspace_file_download(
                &workspace_id,
                completion_access,
                completion_source,
                completion_destination,
                allow_sudo_retry,
                Err(format!("Unable to queue download: {error}")),
            );
        }
    }

    fn finish_workspace_file_download(
        &self,
        workspace_id: &str,
        access: Arc<dyn FileAccess>,
        source: FileNodePath,
        destination: FileDownloadDestination,
        allow_sudo_retry: bool,
        result: Result<Vec<PathBuf>, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.mutation_in_progress.set(false);
        unsafe { files.spinner.stopAnimation(None) };
        files.spinner.setHidden(true);
        files.status.setHidden(!files.rows.borrow().is_empty());
        self.set_page_badge("files", NativePageBadge::None);
        match result {
            Ok(paths) => {
                log::info!(
                    "native Files download completed workspace={} count={}",
                    workspace_id,
                    paths.len()
                );
                self.show_native_toast(&format!(
                    "Downloaded {} {}.",
                    paths.len(),
                    if paths.len() == 1 { "item" } else { "items" }
                ));
                if let Some(path) = paths.first() {
                    files.metadata.setToolTip(Some(&NSString::from_str(&format!(
                        "Downloaded to {}",
                        path.display()
                    ))));
                }
            }
            Err(error) => {
                if allow_sudo_retry && craic_system::system::is_permission_denied_message(&error) {
                    self.offer_file_sudo_retry(
                        access,
                        NativeSudoRetry::Download {
                            source,
                            destination,
                        },
                        "Download Failed",
                        &error,
                    );
                    return;
                }
                files
                    .status
                    .setStringValue(&NSString::from_str("Download failed."));
                files.status.setToolTip(Some(&NSString::from_str(&error)));
                files.status.setHidden(!files.rows.borrow().is_empty());
                self.present_path_action_error("Download Failed", &error);
                log::warn!("native Files download failed workspace={workspace_id}: {error}");
            }
        }
    }

    fn request_file_mutation(&self, access: Arc<dyn FileAccess>, mutation: NativeFileMutation) {
        self.request_file_mutation_with_retry(access, mutation, true);
    }

    fn request_file_mutation_with_retry(
        &self,
        access: Arc<dyn FileAccess>,
        mutation: NativeFileMutation,
        allow_sudo_retry: bool,
    ) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.mutation_in_progress.replace(true) {
            return;
        }
        files.status.setHidden(false);
        files
            .status
            .setStringValue(&NSString::from_str(mutation.progress_label()));
        files.spinner.setHidden(false);
        unsafe { files.spinner.startAnimation(None) };
        self.set_page_badge("files", NativePageBadge::Indicator);
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.finish_file_mutation(
                &workspace_id,
                access.clone(),
                mutation,
                allow_sudo_retry,
                Err("The repository service is unavailable.".to_string()),
            );
            return;
        };
        let completion_access = access.clone();
        if let Err(error) = requests.try_send(RepositoryRequest::RunFileMutation {
            workspace_id: workspace_id.clone(),
            access,
            mutation: mutation.clone(),
            allow_sudo_retry,
            cancellation,
        }) {
            self.finish_file_mutation(
                &workspace_id,
                completion_access,
                mutation,
                allow_sudo_retry,
                Err(format!("Unable to queue file operation: {error}")),
            );
        }
    }

    fn finish_file_mutation(
        &self,
        workspace_id: &str,
        access: Arc<dyn FileAccess>,
        mutation: NativeFileMutation,
        allow_sudo_retry: bool,
        result: Result<Option<FileNodePath>, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.mutation_in_progress.set(false);
        unsafe { files.spinner.stopAnimation(None) };
        files.spinner.setHidden(true);
        self.set_page_badge("files", NativePageBadge::None);
        match result {
            Ok(selected) => {
                let completion_message = match &mutation {
                    NativeFileMutation::CreateFile { .. } => "File created.",
                    NativeFileMutation::CreateDirectory { .. } => "Folder created.",
                    NativeFileMutation::Rename { .. } => "Item renamed.",
                    NativeFileMutation::Copy { .. } => "Item copied.",
                    NativeFileMutation::Move { .. } => "Item moved.",
                    NativeFileMutation::Transfer { .. } => "Item copied.",
                    NativeFileMutation::Upload { sources, .. } => {
                        if sources.len() == 1 {
                            "Item uploaded."
                        } else {
                            "Items uploaded."
                        }
                    }
                    NativeFileMutation::Delete { .. } => "Item deleted.",
                };
                match &mutation {
                    NativeFileMutation::CreateFile { path }
                    | NativeFileMutation::CreateDirectory { path } => {
                        if let Some(parent) = path.parent() {
                            files.expanded.borrow_mut().insert(parent);
                        }
                    }
                    NativeFileMutation::Rename {
                        destination_parent, ..
                    }
                    | NativeFileMutation::Move {
                        destination_parent, ..
                    } => {
                        files
                            .expanded
                            .borrow_mut()
                            .insert(destination_parent.clone());
                    }
                    NativeFileMutation::Copy { destination, .. } => {
                        if let Some(parent) = destination.parent() {
                            files.expanded.borrow_mut().insert(parent);
                        }
                    }
                    NativeFileMutation::Transfer { destination, .. } => {
                        if let Some(parent) = destination.parent() {
                            files.expanded.borrow_mut().insert(parent);
                        }
                    }
                    NativeFileMutation::Upload {
                        destination_parent, ..
                    } => {
                        files
                            .expanded
                            .borrow_mut()
                            .insert(destination_parent.clone());
                    }
                    NativeFileMutation::Delete { .. } => {}
                }
                files.selected_path.replace(selected);
                files.dirty.set(true);
                self.show_native_toast(completion_message);
                self.request_files_tree();
            }
            Err(error) => {
                if allow_sudo_retry && craic_system::system::is_permission_denied_message(&error) {
                    self.offer_file_sudo_retry(
                        access,
                        NativeSudoRetry::Mutation(mutation),
                        "File Operation Failed",
                        &error,
                    );
                    return;
                }
                files
                    .status
                    .setStringValue(&NSString::from_str("File operation failed."));
                files.status.setToolTip(Some(&NSString::from_str(&error)));
                files.status.setHidden(!files.rows.borrow().is_empty());
                self.present_path_action_error("File Operation Failed", &error);
                log::warn!("native file mutation failed workspace={workspace_id}: {error}");
            }
        }
    }

    fn offer_file_sudo_retry(
        &self,
        access: Arc<dyn FileAccess>,
        retry: NativeSudoRetry,
        heading: &str,
        message: &str,
    ) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(heading));
        alert.setInformativeText(&NSString::from_str(&format!(
            "{message}\n\nTry this operation again with sudo?"
        )));
        alert.addButtonWithTitle(&NSString::from_str("Try with sudo"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        alert.setAlertStyle(NSAlertStyle::Warning);
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response == NSAlertFirstButtonReturn {
                delegate.request_file_sudo_authorization(access.clone(), None, retry.clone());
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn request_file_sudo_authorization(
        &self,
        access: Arc<dyn FileAccess>,
        password: Option<FileSudoPassword>,
        retry: NativeSudoRetry,
    ) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.mutation_in_progress.replace(true) {
            return;
        }
        files.status.setHidden(false);
        files
            .status
            .setStringValue(&NSString::from_str("Authorizing file access…"));
        files.spinner.setHidden(false);
        unsafe { files.spinner.startAnimation(None) };
        self.set_page_badge("files", NativePageBadge::Indicator);
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.apply_file_sudo_authorization(
                &workspace_id,
                access.clone(),
                retry,
                Err(FileSudoError::new(
                    FileSudoErrorKind::Unavailable,
                    "The repository service is unavailable.",
                )),
            );
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::AuthorizeFileSudo {
            workspace_id: workspace_id.clone(),
            access: access.clone(),
            password,
            retry: retry.clone(),
        }) {
            self.apply_file_sudo_authorization(
                &workspace_id,
                access,
                retry,
                Err(FileSudoError::new(
                    FileSudoErrorKind::Unavailable,
                    format!("Unable to queue sudo authorization: {error}"),
                )),
            );
        }
    }

    fn apply_file_sudo_authorization(
        &self,
        workspace_id: &str,
        access: Arc<dyn FileAccess>,
        retry: NativeSudoRetry,
        result: Result<Arc<dyn FileAccess>, FileSudoError>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.mutation_in_progress.set(false);
        unsafe { files.spinner.stopAnimation(None) };
        files.spinner.setHidden(true);
        files.status.setHidden(!files.rows.borrow().is_empty());
        self.set_page_badge("files", NativePageBadge::None);
        match result {
            Ok(sudo_access) => match retry {
                NativeSudoRetry::Mutation(mutation) => {
                    self.request_file_mutation_with_retry(sudo_access, mutation, false);
                }
                NativeSudoRetry::Download {
                    source,
                    destination,
                } => {
                    self.request_workspace_file_download_with_retry(
                        sudo_access,
                        source,
                        destination,
                        false,
                    );
                }
                NativeSudoRetry::Save {
                    path,
                    text,
                    expected_signature,
                    edit_generation,
                } => {
                    self.request_workspace_file_save_with_retry(
                        sudo_access,
                        path,
                        text,
                        expected_signature,
                        edit_generation,
                        false,
                    );
                }
            },
            Err(error)
                if matches!(
                    error.kind,
                    FileSudoErrorKind::PasswordRequired | FileSudoErrorKind::AuthenticationFailed
                ) =>
            {
                self.prompt_file_sudo_password(access, retry, &error.message);
            }
            Err(error) => {
                self.present_path_action_error("Sudo Authorization Failed", &error.message);
                log::warn!(
                    "native Files sudo authorization failed workspace={workspace_id}: {}",
                    error.message
                );
            }
        }
    }

    fn prompt_file_sudo_password(
        &self,
        access: Arc<dyn FileAccess>,
        retry: NativeSudoRetry,
        message: &str,
    ) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Sudo Authentication"));
        alert.setInformativeText(&NSString::from_str(if message.is_empty() {
            "Enter the sudo password for this workspace."
        } else {
            message
        }));
        alert.addButtonWithTitle(&NSString::from_str("Authenticate"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let password = NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, NSSize::new(360.0, 24.0)),
        );
        password.setPlaceholderString(Some(&NSString::from_str("Password")));
        alert.setAccessoryView(Some(&password));
        let delegate = self.retain();
        let completion_password = password.clone();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn {
                return;
            }
            let bytes = completion_password.stringValue().to_string().into_bytes();
            completion_password.setStringValue(&NSString::new());
            delegate.request_file_sudo_authorization(
                access.clone(),
                Some(FileSudoPassword::new(bytes)),
                retry.clone(),
            );
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
        window.makeFirstResponder(Some(&password));
    }

}
