impl AppDelegate {
    fn filtered_file_tree_rows(&self) -> Vec<NativeFileRow> {
        let Some(files) = self.ivars().files.get() else {
            return Vec::new();
        };
        let query = files.query.borrow();
        files
            .rows
            .borrow()
            .iter()
            .filter(|row| {
                query.is_empty()
                    || row
                        .info
                        .display_name
                        .to_lowercase()
                        .contains(query.as_str())
                    || row
                        .info
                        .path
                        .display()
                        .to_lowercase()
                        .contains(query.as_str())
            })
            .cloned()
            .collect()
    }

    fn filtered_container_rows(&self) -> Vec<NativeContainerRow> {
        let Some(containers) = self.ivars().containers.get() else {
            return Vec::new();
        };
        let query = containers.query.borrow().trim().to_lowercase();
        let rows = containers.rows.borrow();
        let search_active = !query.is_empty();
        let expanded_groups = containers.expanded_groups.borrow();

        let mut filtered = Vec::new();
        let mut index = 0;
        while index < rows.len() {
            let NativeContainerRow::Group(group) = &rows[index] else {
                if let NativeContainerRow::Container(container) = &rows[index]
                    && (!search_active || docker::container_matches(container, &query))
                {
                    filtered.push(rows[index].clone());
                }
                index += 1;
                continue;
            };
            index += 1;
            let group_matches = !search_active || docker::group_matches(group, &query);
            let mut matching_containers = Vec::new();
            while index < rows.len() {
                let NativeContainerRow::Container(container) = &rows[index] else {
                    break;
                };
                if group_matches || docker::container_matches(container, &query) {
                    matching_containers.push(container.clone());
                }
                index += 1;
            }
            if group_matches || !matching_containers.is_empty() {
                let mut filtered_group = group.clone();
                filtered_group.containers = matching_containers.clone();
                filtered.push(NativeContainerRow::Group(filtered_group));
                if search_active || expanded_groups.contains(&group.key) {
                    filtered.extend(
                        matching_containers
                            .into_iter()
                            .map(NativeContainerRow::Container),
                    );
                }
            }
        }
        filtered
    }

    fn make_container_cell(
        &self,
        table: &NSTableView,
        column: Option<&NSTableColumn>,
        row: usize,
    ) -> Option<Retained<NSView>> {
        let row = self.filtered_container_rows().get(row).cloned()?;
        // Inset-style source-list rows are placed one native horizontal inset from either edge.
        // Floating group rows have no column, so give both row kinds the same effective width.
        let width = column
            .map(NSTableColumn::width)
            .unwrap_or_else(|| {
                table.bounds().size.width - CONTAINER_SOURCE_LIST_HORIZONTAL_INSET * 2.0
            })
            .max(1.0);
        let cell = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, CONTAINER_ROW_HEIGHT),
            ),
        );
        cell.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        match row {
            NativeContainerRow::Group(group) => {
                let expanded = self.ivars().containers.get().is_some_and(|containers| {
                    !containers.query.borrow().trim().is_empty()
                        || containers.expanded_groups.borrow().contains(&group.key)
                });
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(if expanded {
                        "chevron.down"
                    } else {
                        "chevron.right"
                    }),
                    Some(&NSString::from_str(if expanded {
                        "Collapse group"
                    } else {
                        "Expand group"
                    })),
                ) {
                    let disclosure = NSImageView::imageViewWithImage(&image, self.mtm());
                    disclosure.setTranslatesAutoresizingMaskIntoConstraints(false);
                    disclosure.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
                    cell.addSubview(&disclosure);
                    disclosure
                        .leadingAnchor()
                        .constraintEqualToAnchor_constant(&cell.leadingAnchor(), 10.0)
                        .setActive(true);
                    disclosure
                        .centerYAnchor()
                        .constraintEqualToAnchor(&cell.centerYAnchor())
                        .setActive(true);
                    disclosure
                        .widthAnchor()
                        .constraintEqualToConstant(12.0)
                        .setActive(true);
                    disclosure
                        .heightAnchor()
                        .constraintEqualToConstant(12.0)
                        .setActive(true);
                }
                let group_icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(if group.is_compose() {
                        "shippingbox"
                    } else {
                        "internaldrive"
                    }),
                    Some(&NSString::from_str(if group.is_compose() {
                        "Compose project"
                    } else {
                        "Individual containers"
                    })),
                )
                .map(|image| {
                    let icon = NSImageView::imageViewWithImage(&image, self.mtm());
                    icon.setTranslatesAutoresizingMaskIntoConstraints(false);
                    icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
                    cell.addSubview(&icon);
                    icon.leadingAnchor()
                        .constraintEqualToAnchor_constant(&cell.leadingAnchor(), 30.0)
                        .setActive(true);
                    icon.centerYAnchor()
                        .constraintEqualToAnchor(&cell.centerYAnchor())
                        .setActive(true);
                    icon.widthAnchor()
                        .constraintEqualToConstant(18.0)
                        .setActive(true);
                    icon.heightAnchor()
                        .constraintEqualToConstant(18.0)
                        .setActive(true);
                    icon
                });
                let label =
                    NSTextField::labelWithString(&NSString::from_str(&group.title), self.mtm());
                label.setTranslatesAutoresizingMaskIntoConstraints(false);
                label.setFont(Some(&NSFont::boldSystemFontOfSize(12.5)));
                label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                label.setMaximumNumberOfLines(1);
                label.setContentCompressionResistancePriority_forOrientation(
                    NSLayoutPriorityDefaultLow,
                    NSLayoutConstraintOrientation::Horizontal,
                );
                if let Some(cell) = label.cell() {
                    cell.setUsesSingleLineMode(true);
                    cell.setTruncatesLastVisibleLine(true);
                }
                cell.addSubview(&label);
                label
                    .leadingAnchor()
                    .constraintEqualToAnchor_constant(&cell.leadingAnchor(), 54.0)
                    .setActive(true);
                label
                    .centerYAnchor()
                    .constraintEqualToAnchor(&cell.centerYAnchor())
                    .setActive(true);
                let count = NSTextField::labelWithString(
                    &NSString::from_str(&group.containers.len().to_string()),
                    self.mtm(),
                );
                count.setTranslatesAutoresizingMaskIntoConstraints(false);
                count.setAlignment(NSTextAlignment::Right);
                count.setTextColor(Some(&NSColor::secondaryLabelColor()));
                count.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                count.setContentCompressionResistancePriority_forOrientation(
                    NSLayoutPriorityDefaultHigh,
                    NSLayoutConstraintOrientation::Horizontal,
                );
                cell.addSubview(&count);
                count
                    .trailingAnchor()
                    .constraintEqualToAnchor_constant(
                        &cell.trailingAnchor(),
                        -(CONTAINER_SOURCE_LIST_HORIZONTAL_INSET + CONTAINER_ROW_TRAILING_INSET),
                    )
                    .setActive(true);
                count
                    .centerYAnchor()
                    .constraintEqualToAnchor(&cell.centerYAnchor())
                    .setActive(true);
                label
                    .trailingAnchor()
                    .constraintLessThanOrEqualToAnchor_constant(&count.leadingAnchor(), -8.0)
                    .setActive(true);
                drop(group_icon);
            }
            NativeContainerRow::Container(container) => {
                let running = docker::state_is_running(&container.state);
                let state_icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(if running {
                        "play.circle.fill"
                    } else {
                        "stop.circle"
                    }),
                    Some(&NSString::from_str(&container.state)),
                )
                .map(|image| {
                    let icon = NSImageView::imageViewWithImage(&image, self.mtm());
                    icon.setTranslatesAutoresizingMaskIntoConstraints(false);
                    let color = if running {
                        NSColor::systemGreenColor()
                    } else {
                        NSColor::secondaryLabelColor()
                    };
                    icon.setContentTintColor(Some(&color));
                    cell.addSubview(&icon);
                    icon.leadingAnchor()
                        .constraintEqualToAnchor_constant(&cell.leadingAnchor(), 16.0)
                        .setActive(true);
                    icon.centerYAnchor()
                        .constraintEqualToAnchor(&cell.centerYAnchor())
                        .setActive(true);
                    icon.widthAnchor()
                        .constraintEqualToConstant(18.0)
                        .setActive(true);
                    icon.heightAnchor()
                        .constraintEqualToConstant(18.0)
                        .setActive(true);
                    icon
                });
                let title = NSTextField::labelWithString(
                    &NSString::from_str(container.display_name()),
                    self.mtm(),
                );
                title.setTranslatesAutoresizingMaskIntoConstraints(false);
                title.setFont(Some(&NSFont::systemFontOfSize(12.5)));
                title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                title.setMaximumNumberOfLines(1);
                title.setContentCompressionResistancePriority_forOrientation(
                    NSLayoutPriorityDefaultLow,
                    NSLayoutConstraintOrientation::Horizontal,
                );
                if let Some(cell) = title.cell() {
                    cell.setUsesSingleLineMode(true);
                    cell.setTruncatesLastVisibleLine(true);
                }
                cell.addSubview(&title);
                title
                    .leadingAnchor()
                    .constraintEqualToAnchor_constant(&cell.leadingAnchor(), 42.0)
                    .setActive(true);
                title
                    .centerYAnchor()
                    .constraintEqualToAnchor(&cell.centerYAnchor())
                    .setActive(true);
                let state =
                    NSTextField::labelWithString(&NSString::from_str(&container.state), self.mtm());
                state.setTranslatesAutoresizingMaskIntoConstraints(false);
                state.setAlignment(NSTextAlignment::Right);
                state.setFont(Some(&NSFont::systemFontOfSize(10.5)));
                state.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                state.setMaximumNumberOfLines(1);
                state.setContentCompressionResistancePriority_forOrientation(
                    NSLayoutPriorityDefaultHigh,
                    NSLayoutConstraintOrientation::Horizontal,
                );
                if let Some(cell) = state.cell() {
                    cell.setUsesSingleLineMode(true);
                    cell.setTruncatesLastVisibleLine(true);
                }
                state.setToolTip(Some(&NSString::from_str(&container.state)));
                let color = if running {
                    NSColor::systemGreenColor()
                } else {
                    NSColor::secondaryLabelColor()
                };
                state.setTextColor(Some(&color));
                cell.addSubview(&state);
                state
                    .trailingAnchor()
                    .constraintEqualToAnchor_constant(
                        &cell.trailingAnchor(),
                        -CONTAINER_ROW_TRAILING_INSET,
                    )
                    .setActive(true);
                state
                    .centerYAnchor()
                    .constraintEqualToAnchor(&cell.centerYAnchor())
                    .setActive(true);
                state
                    .widthAnchor()
                    .constraintLessThanOrEqualToConstant(CONTAINER_STATE_MAX_WIDTH)
                    .setActive(true);
                title
                    .trailingAnchor()
                    .constraintLessThanOrEqualToAnchor_constant(&state.leadingAnchor(), -8.0)
                    .setActive(true);
                drop(state_icon);
            }
        }
        Some(cell)
    }

    fn select_container_row(&self, row: usize) {
        let Some(row) = self.filtered_container_rows().get(row).cloned() else {
            return;
        };
        self.display_container_row(row, true);
    }

    fn render_container_detail_sections(
        &self,
        show_inspect: bool,
        sections: Vec<(String, Vec<(String, String)>)>,
    ) {
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        let subviews = containers.details_content.subviews();
        for index in 0..subviews.count() {
            subviews.objectAtIndex(index).removeFromSuperview();
        }

        let viewport = containers.details_scroll.contentSize();
        let rows = sections.iter().map(|(_, rows)| rows.len()).sum::<usize>();
        let intrinsic_height = 24.0
            + if show_inspect { 44.0 } else { 0.0 }
            + sections.len() as f64 * 42.0
            + rows as f64 * 28.0;
        let width = viewport.width.max(1.0);
        let height = intrinsic_height.max(viewport.height).max(1.0);
        containers
            .details_content
            .setFrameSize(NSSize::new(width, height));

        let mut cursor = height - 12.0;
        if show_inspect {
            cursor -= 28.0;
            containers.inspect.setFrame(NSRect::new(
                NSPoint::new(12.0, cursor),
                NSSize::new(86.0, 28.0),
            ));
            containers.inspect.setHidden(false);
            containers.details_content.addSubview(&containers.inspect);
            cursor -= 16.0;
        } else {
            containers.inspect.setHidden(true);
        }

        for (section, rows) in sections {
            cursor -= 22.0;
            let heading = NSTextField::labelWithString(&NSString::from_str(&section), self.mtm());
            heading.setFrame(NSRect::new(
                NSPoint::new(12.0, cursor),
                NSSize::new((width - 24.0).max(1.0), 20.0),
            ));
            heading.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            heading.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            heading.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            containers.details_content.addSubview(&heading);
            cursor -= 8.0;

            let separator = NSBox::initWithFrame(
                NSBox::alloc(self.mtm()),
                NSRect::new(
                    NSPoint::new(12.0, cursor),
                    NSSize::new((width - 24.0).max(1.0), 1.0),
                ),
            );
            separator.setBoxType(NSBoxType::Separator);
            separator.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            containers.details_content.addSubview(&separator);

            for (key, value) in rows {
                cursor -= 28.0;
                let key_width = (width * 0.3).clamp(92.0, 180.0);
                let key_label = NSTextField::labelWithString(&NSString::from_str(&key), self.mtm());
                key_label.setFrame(NSRect::new(
                    NSPoint::new(12.0, cursor),
                    NSSize::new((key_width - 12.0).max(1.0), 20.0),
                ));
                key_label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                key_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                key_label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                key_label.setToolTip(Some(&NSString::from_str(&key)));
                containers.details_content.addSubview(&key_label);

                let value_label =
                    NSTextField::labelWithString(&NSString::from_str(&value), self.mtm());
                value_label.setFrame(NSRect::new(
                    NSPoint::new(key_width, cursor),
                    NSSize::new((width - key_width - 12.0).max(1.0), 20.0),
                ));
                value_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                value_label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                value_label.setSelectable(true);
                value_label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
                value_label.setMaximumNumberOfLines(1);
                if let Some(cell) = value_label.cell() {
                    cell.setUsesSingleLineMode(true);
                    cell.setTruncatesLastVisibleLine(true);
                }
                value_label.setToolTip(Some(&NSString::from_str(&value)));
                containers.details_content.addSubview(&value_label);
            }
            cursor -= 12.0;
        }
        let clip = containers.details_scroll.contentView();
        clip.scrollToPoint(NSPoint::new(0.0, (height - viewport.height).max(0.0)));
        containers.details_scroll.reflectScrolledClipView(&clip);
    }

    fn display_container_row(&self, row: NativeContainerRow, toggle_group: bool) {
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        containers.inspect_code.setHidden(true);
        match row {
            NativeContainerRow::Group(filtered_group) => {
                let group = containers
                    .rows
                    .borrow()
                    .iter()
                    .find_map(|row| match row {
                        NativeContainerRow::Group(group) if group.key == filtered_group.key => {
                            Some(group.clone())
                        }
                        _ => None,
                    })
                    .unwrap_or(filtered_group);
                containers.selected_id.borrow_mut().take();
                containers
                    .selected_group_key
                    .replace(Some(group.key.clone()));
                if toggle_group && containers.query.borrow().trim().is_empty() {
                    let mut expanded = containers.expanded_groups.borrow_mut();
                    if !expanded.remove(&group.key) {
                        expanded.insert(group.key.clone());
                    }
                    drop(expanded);
                    containers.table.reloadData();
                }
                containers
                    .title
                    .setStringValue(&NSString::from_str(&group.title));
                let running = group
                    .containers
                    .iter()
                    .filter(|container| docker::state_is_running(&container.state))
                    .count();
                let stopped = group.containers.len().saturating_sub(running);
                containers
                    .subtitle
                    .setStringValue(&NSString::from_str(&format!(
                        "{} containers",
                        group.containers.len()
                    )));
                let mut overview = vec![
                    ("Containers".to_string(), group.containers.len().to_string()),
                    ("Running".to_string(), running.to_string()),
                    ("Stopped".to_string(), stopped.to_string()),
                ];
                if let Some(compose) = group.compose_metadata() {
                    overview.extend([
                        ("Project".to_string(), compose.project.clone()),
                        (
                            "Working Directory".to_string(),
                            compose
                                .working_dir
                                .clone()
                                .unwrap_or_else(|| "Unknown".into()),
                        ),
                        (
                            "Compose Files".to_string(),
                            if compose.config_files.is_empty() {
                                "—".to_string()
                            } else {
                                compose.config_files.join(", ")
                            },
                        ),
                        (
                            "Environment File".to_string(),
                            compose
                                .environment_file
                                .clone()
                                .unwrap_or_else(|| "None".into()),
                        ),
                    ]);
                }
                let services = group
                    .containers
                    .iter()
                    .map(|container| {
                        (
                            container
                                .service
                                .clone()
                                .unwrap_or_else(|| container.display_name().to_string()),
                            format!("{} · {}", container.display_name(), container.status),
                        )
                    })
                    .collect::<Vec<_>>();
                let mut ports = Vec::new();
                let mut networks = Vec::new();
                for container in &group.containers {
                    if !container.ports.trim().is_empty() && !ports.contains(&container.ports) {
                        ports.push(container.ports.clone());
                    }
                    for network in &container.networks {
                        if !networks.contains(network) {
                            networks.push(network.clone());
                        }
                    }
                }
                self.render_container_detail_sections(
                    false,
                    vec![
                        ("Overview".to_string(), overview),
                        ("Services".to_string(), services),
                        (
                            "Aggregate".to_string(),
                            vec![
                                (
                                    "Ports".to_string(),
                                    if ports.is_empty() {
                                        "—".to_string()
                                    } else {
                                        ports.join(", ")
                                    },
                                ),
                                (
                                    "Networks".to_string(),
                                    if networks.is_empty() {
                                        "—".to_string()
                                    } else {
                                        networks.join(", ")
                                    },
                                ),
                            ],
                        ),
                    ],
                );
                containers.details_scroll.setHidden(false);
                containers.empty.setHidden(true);
                self.update_container_actions();
            }
            NativeContainerRow::Container(container) => {
                containers.selected_group_key.borrow_mut().take();
                containers.selected_id.replace(Some(container.id.clone()));
                containers
                    .title
                    .setStringValue(&NSString::from_str(container.display_name()));
                containers
                    .subtitle
                    .setStringValue(&NSString::from_str(&container.image));
                let labels = if container.labels.is_empty() {
                    vec![("Entries".to_string(), "—".to_string())]
                } else {
                    container
                        .labels
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>()
                };
                self.render_container_detail_sections(
                    true,
                    vec![
                        (
                            "Overview".to_string(),
                            vec![
                                ("Name".to_string(), container.display_name().to_string()),
                                ("ID".to_string(), container.short_id().to_string()),
                                ("Image".to_string(), container.image.clone()),
                                ("State".to_string(), container.state.clone()),
                                ("Status".to_string(), container.status.clone()),
                                ("Created".to_string(), container.created_at.clone()),
                                ("Running For".to_string(), container.running_for.clone()),
                                (
                                    "Ports".to_string(),
                                    if container.ports.is_empty() {
                                        "—".to_string()
                                    } else {
                                        container.ports.clone()
                                    },
                                ),
                            ],
                        ),
                        (
                            "Networks And Mounts".to_string(),
                            vec![
                                (
                                    "Networks".to_string(),
                                    if container.networks.is_empty() {
                                        "—".to_string()
                                    } else {
                                        container.networks.join(", ")
                                    },
                                ),
                                (
                                    "Mounts".to_string(),
                                    if container.mounts.is_empty() {
                                        "—".to_string()
                                    } else {
                                        container.mounts.join(", ")
                                    },
                                ),
                            ],
                        ),
                        ("Labels".to_string(), labels),
                    ],
                );
                containers.details_scroll.setHidden(false);
                containers.empty.setHidden(true);
                self.update_container_actions();
            }
        }
    }

    fn selected_container(&self) -> Option<ContainerSummary> {
        let containers = self.ivars().containers.get()?;
        let selected_id = containers.selected_id.borrow();
        let selected_id = selected_id.as_deref()?;
        containers.rows.borrow().iter().find_map(|row| match row {
            NativeContainerRow::Container(container) if container.id == selected_id => {
                Some(container.clone())
            }
            _ => None,
        })
    }

    fn prepare_container_menu_for_row(&self, row: usize) -> Option<Retained<NSMenu>> {
        let containers = self.ivars().containers.get()?;
        let row_data = self.filtered_container_rows().get(row).cloned()?;
        containers.context_selection.set(true);
        containers
            .table
            .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
        if containers.context_selection.replace(false) {
            self.display_container_row(row_data.clone(), false);
        }

        let sections = match row_data {
            NativeContainerRow::Group(group) if group.is_compose() => vec![
                vec![("Compose Logs", sel!(showContainerLogs:), false)],
                vec![
                    ("Compose Start", sel!(startContainer:), false),
                    ("Compose Stop", sel!(stopContainer:), false),
                    ("Compose Restart", sel!(restartContainer:), false),
                ],
                vec![("Compose Down", sel!(removeContainer:), true)],
            ],
            NativeContainerRow::Group(_) => return None,
            NativeContainerRow::Container(_) => vec![
                vec![
                    ("View Logs", sel!(showContainerLogs:), false),
                    ("Attach Shell", sel!(attachContainerShell:), false),
                    ("Inspect", sel!(inspectContainer:), false),
                ],
                vec![
                    ("Start", sel!(startContainer:), false),
                    ("Stop", sel!(stopContainer:), false),
                    ("Restart", sel!(restartContainer:), false),
                ],
                vec![("Remove", sel!(removeContainer:), true)],
            ],
        };

        let menu = containers.menu.clone();
        menu.removeAllItems();
        for section in sections {
            if menu.numberOfItems() > 0 {
                menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            }
            for (title, action, destructive) in section {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(self.mtm()),
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::new(),
                    )
                };
                unsafe { item.setTarget(Some(self)) };
                if destructive
                    && let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str("trash"),
                        Some(&NSString::from_str("Destructive container action")),
                    )
                {
                    item.setImage(Some(&image));
                }
                menu.addItem(&item);
            }
        }
        Some(menu)
    }

    fn selected_container_group(&self) -> Option<ContainerGroup> {
        let containers = self.ivars().containers.get()?;
        let selected_key = containers.selected_group_key.borrow();
        let selected_key = selected_key.as_deref()?;
        containers.rows.borrow().iter().find_map(|row| match row {
            NativeContainerRow::Group(group) if group.key == selected_key => Some(group.clone()),
            _ => None,
        })
    }

    fn selected_compose_project(&self) -> Option<ComposeProject> {
        self.selected_container_group()?.compose
    }

    fn update_container_actions(&self) {
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        let selected = self.selected_container();
        let compose = self.selected_compose_project();
        let available = selected.is_some() || compose.is_some();
        let action_available = !containers.action_in_progress.get();
        containers.logs.setEnabled(available);
        containers.inspect.setEnabled(selected.is_some());
        containers.shell.setEnabled(
            selected
                .as_ref()
                .is_some_and(|container| docker::state_is_running(&container.state)),
        );
        containers.start.setEnabled(
            action_available
                && (compose.is_some()
                    || available && selected.as_ref().is_some_and(ContainerSummary::can_start)),
        );
        containers.stop.setEnabled(
            action_available
                && (compose.is_some()
                    || available && selected.as_ref().is_some_and(ContainerSummary::can_stop)),
        );
        containers.restart.setEnabled(
            action_available
                && (compose.is_some()
                    || available && selected.as_ref().is_some_and(ContainerSummary::can_restart)),
        );
        containers.remove.setEnabled(
            action_available
                && (compose.is_some()
                    || available && selected.as_ref().is_some_and(ContainerSummary::can_remove)),
        );
        containers
            .remove
            .setTitle(&NSString::from_str(if compose.is_some() {
                "Down"
            } else {
                "Remove"
            }));
    }

    fn active_docker_access(&self) -> Result<Arc<dyn DockerAccess>, String> {
        let workspace_id = self
            .ivars()
            .active_workspace_id
            .borrow()
            .clone()
            .ok_or_else(|| "Open a workspace to use Docker.".to_string())?;
        let workspace = self
            .ivars()
            .workspaces
            .borrow()
            .iter()
            .find(|entry| entry.selection_id() == workspace_id)
            .map(|entry| entry.workspace.clone())
            .ok_or_else(|| "The active workspace is unavailable.".to_string())?;
        docker_access_for_workspace(&workspace)
    }

    fn request_container_detail(&self, kind: ContainerDetailKind) {
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        let Some(container) = self.selected_container() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let access = match self.active_docker_access() {
            Ok(access) => access,
            Err(error) => {
                self.present_path_action_error("Docker Unavailable", &error);
                return;
            }
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let request_id = containers.detail_request_id.get().wrapping_add(1);
        containers.detail_request_id.set(request_id);
        containers
            .title
            .setStringValue(&NSString::from_str(&format!(
                "Inspect {}",
                container.display_name()
            )));
        containers
            .subtitle
            .setStringValue(&NSString::from_str(&container.image));
        containers.inspect_code.set_document(
            "docker-inspect.json",
            "Loading inspect payload…".to_string(),
            Vec::new(),
            craic_render_skia::EditorSelection::default(),
            false,
            true,
        );
        containers.details_scroll.setHidden(true);
        containers.inspect_code.setHidden(false);
        containers.inspect.setHidden(true);
        containers.empty.setHidden(true);
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        let request = RepositoryRequest::LoadContainerDetail {
            workspace_id,
            access,
            container_id: container.id,
            request_id,
            kind,
            cancellation,
        };
        if let Err(error) = requests.try_send(request) {
            containers.inspect_code.set_document(
                "docker-inspect.json",
                format!("Unable to queue Docker inspect: {error}"),
                Vec::new(),
                craic_render_skia::EditorSelection::default(),
                false,
                true,
            );
        }
    }

    fn apply_container_detail(
        &self,
        workspace_id: &str,
        container_id: &str,
        request_id: u64,
        kind: ContainerDetailKind,
        result: Result<String, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        if containers.detail_request_id.get() != request_id
            || containers.selected_id.borrow().as_deref() != Some(container_id)
        {
            log::debug!(
                "discarding stale container detail workspace={workspace_id} container={container_id}"
            );
            return;
        }
        debug_assert_eq!(kind, ContainerDetailKind::Inspect);
        let container = self.selected_container();
        if let Some(container) = container.as_ref() {
            containers
                .title
                .setStringValue(&NSString::from_str(&format!(
                    "Inspect {}",
                    container.display_name()
                )));
            containers
                .subtitle
                .setStringValue(&NSString::from_str(&container.image));
        }
        match result {
            Ok(text) => {
                containers.inspect_code.set_document(
                    "docker-inspect.json",
                    text,
                    Vec::new(),
                    craic_render_skia::EditorSelection::default(),
                    false,
                    true,
                );
            }
            Err(error) => {
                containers.inspect_code.set_document(
                    "docker-inspect.json",
                    format!("Docker inspect failed:\n\n{error}"),
                    Vec::new(),
                    craic_render_skia::EditorSelection::default(),
                    false,
                    true,
                );
                log::warn!(
                    "native container detail failed workspace={workspace_id} container={container_id}: {error}"
                );
            }
        }
        containers.details_scroll.setHidden(true);
        containers.inspect_code.setHidden(false);
        containers.empty.setHidden(true);
    }

    fn request_container_action(&self, action: docker::ContainerAction) {
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        if containers.action_in_progress.get() {
            log::debug!(
                "native container lifecycle request ignored while another action is active"
            );
            return;
        }
        let container = self.selected_container();
        let compose = self.selected_compose_project();
        if container.is_none() && compose.is_none() {
            return;
        }
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let access = match self.active_docker_access() {
            Ok(access) => access,
            Err(error) => {
                self.present_path_action_error("Docker Unavailable", &error);
                return;
            }
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let workspace_generation = self.ivars().workspace_generation.get();
        let request_id = containers.action_request_id.get().wrapping_add(1);
        containers.action_request_id.set(request_id);
        containers.action_in_progress.set(true);
        self.update_container_actions();
        containers
            .subtitle
            .setStringValue(&NSString::from_str("Working…"));
        let Some(requests) = self.ivars().repository_requests.get() else {
            containers.action_in_progress.set(false);
            self.update_container_actions();
            return;
        };
        let request = if let Some(container) = container {
            RepositoryRequest::RunContainerAction {
                workspace_id,
                workspace_generation,
                access,
                container_id: container.id,
                action,
                request_id,
                cancellation,
            }
        } else if let Some(compose) = compose {
            let action = match action {
                docker::ContainerAction::Start => docker::ComposeAction::Start,
                docker::ContainerAction::Stop => docker::ComposeAction::Stop,
                docker::ContainerAction::Restart => docker::ComposeAction::Restart,
                docker::ContainerAction::Remove => docker::ComposeAction::Down,
            };
            RepositoryRequest::RunComposeAction {
                workspace_id,
                workspace_generation,
                access,
                compose,
                action,
                request_id,
                cancellation,
            }
        } else {
            unreachable!("container action requires a selected container or Compose project")
        };
        if let Err(error) = requests.try_send(request) {
            containers.action_in_progress.set(false);
            self.update_container_actions();
            self.present_path_action_error(
                "Docker Action Failed",
                &format!("Unable to queue Docker action: {error}"),
            );
        }
    }

    fn finish_container_action(
        &self,
        workspace_id: &str,
        workspace_generation: craic_app_core::Generation,
        request_id: u64,
        result: Result<String, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || self.ivars().workspace_generation.get() != workspace_generation
        {
            log::debug!(
                "discarding container action from stale workspace generation workspace={workspace_id} generation={}",
                workspace_generation.get()
            );
            return;
        }
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        if containers.action_request_id.get() != request_id {
            log::debug!(
                "discarding stale container action workspace={workspace_id} request={request_id}"
            );
            return;
        }
        containers.action_in_progress.set(false);
        self.update_container_actions();
        match result {
            Ok(message) => {
                containers
                    .subtitle
                    .setStringValue(&NSString::from_str(&message));
                self.show_native_toast(&message);
                self.request_containers();
            }
            Err(error) => self.present_path_action_error("Docker Action Failed", &error),
        }
    }

    fn request_containers(&self) {
        let Some(containers) = self.ivars().containers.get() else {
            self.complete_pending_page_service(
                "containers",
                Err("The Containers page is unavailable".to_string()),
            );
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            self.complete_pending_page_service(
                "containers",
                Err("No workspace is active".to_string()),
            );
            return;
        };
        let Some(workspace) = self
            .ivars()
            .workspaces
            .borrow()
            .iter()
            .find(|entry| entry.selection_id() == workspace_id)
            .map(|entry| entry.workspace.clone())
        else {
            self.complete_pending_page_service(
                "containers",
                Err("The active workspace is unavailable".to_string()),
            );
            return;
        };
        let access = match docker_access_for_workspace(&workspace) {
            Ok(access) => access,
            Err(error) => {
                self.show_containers_error(&error);
                self.complete_pending_page_service("containers", Err(error));
                return;
            }
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            self.complete_pending_page_service(
                "containers",
                Err("The workspace is shutting down".to_string()),
            );
            return;
        };
        let generation = containers.generation.get().wrapping_add(1);
        containers.generation.set(generation);
        containers.loading.set(true);
        containers.dirty.set(false);
        containers.scroll.setHidden(true);
        containers.status.setHidden(false);
        containers.status.setToolTip(None);
        containers
            .status
            .setStringValue(&NSString::from_str("Loading containers…"));
        containers.spinner.setHidden(false);
        unsafe { containers.spinner.startAnimation(None) };
        self.set_page_badge("containers", NativePageBadge::Indicator);
        let Some(requests) = self.ivars().repository_requests.get() else {
            containers.loading.set(false);
            unsafe { containers.spinner.stopAnimation(None) };
            containers.spinner.setHidden(true);
            self.set_page_badge("containers", NativePageBadge::None);
            self.show_containers_error("The repository service is unavailable.");
            self.complete_pending_page_service(
                "containers",
                Err("The repository service is unavailable".to_string()),
            );
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadContainers {
            workspace_id,
            access,
            generation,
            cancellation,
        }) {
            containers.loading.set(false);
            unsafe { containers.spinner.stopAnimation(None) };
            containers.spinner.setHidden(true);
            containers
                .status
                .setStringValue(&NSString::from_str(&format!(
                    "Unable to queue container loading: {error}"
                )));
            self.set_page_badge("containers", NativePageBadge::None);
            self.show_containers_error(&format!("Unable to queue container loading: {error}"));
            self.complete_pending_page_service("containers", Err(error.to_string()));
        }
    }

    fn apply_containers(
        &self,
        workspace_id: &str,
        generation: u64,
        result: Result<ContainerInventory, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        if containers.generation.get() != generation {
            log::debug!("discarding stale container inventory workspace={workspace_id}");
            return;
        }
        let page_service_result = result
            .as_ref()
            .map(|inventory| serde_json::json!({ "containers": inventory.container_count() }))
            .map_err(Clone::clone);
        containers.loading.set(false);
        unsafe { containers.spinner.stopAnimation(None) };
        containers.spinner.setHidden(true);
        self.set_page_badge("containers", NativePageBadge::None);
        match result {
            Ok(inventory) => {
                let empty = inventory.container_count() == 0;
                let mut rows = Vec::new();
                let mut group_keys = HashSet::new();
                for group in inventory.groups {
                    group_keys.insert(group.key.clone());
                    if group.is_compose() {
                        containers
                            .expanded_groups
                            .borrow_mut()
                            .insert(group.key.clone());
                    }
                    rows.push(NativeContainerRow::Group(group.clone()));
                    rows.extend(
                        group
                            .containers
                            .into_iter()
                            .map(NativeContainerRow::Container),
                    );
                }
                if empty {
                    rows.clear();
                    group_keys.clear();
                }
                containers
                    .expanded_groups
                    .borrow_mut()
                    .retain(|key| group_keys.contains(key));
                containers.rows.replace(rows);
                let selected_container_missing = containers
                    .selected_id
                    .borrow()
                    .as_deref()
                    .is_some_and(|selected| {
                        !containers.rows.borrow().iter().any(|row| {
                            matches!(
                                row,
                                NativeContainerRow::Container(container)
                                    if container.id == selected
                            )
                        })
                    });
                let selected_group_missing = containers
                    .selected_group_key
                    .borrow()
                    .as_deref()
                    .is_some_and(|selected| {
                        !containers.rows.borrow().iter().any(|row| {
                            matches!(
                                row,
                                NativeContainerRow::Group(group) if group.key == selected
                            )
                        })
                    });
                if selected_container_missing || selected_group_missing {
                    containers.selected_id.borrow_mut().take();
                    containers.selected_group_key.borrow_mut().take();
                    containers.details_scroll.setHidden(true);
                    containers.inspect_code.setHidden(true);
                    containers.inspect.setHidden(true);
                    containers.empty.setHidden(false);
                }
                containers.table.reloadData();
                containers.scroll.setHidden(empty);
                containers.status.setToolTip(None);
                containers.status.setHidden(!empty);
                if empty {
                    containers
                        .status
                        .setStringValue(&NSString::from_str("No containers."));
                }
                let selected_row = containers
                    .selected_id
                    .borrow()
                    .as_deref()
                    .and_then(|selected| {
                        containers.rows.borrow().iter().find_map(|row| match row {
                            NativeContainerRow::Container(container)
                                if container.id == selected =>
                            {
                                Some(row.clone())
                            }
                            _ => None,
                        })
                    })
                    .or_else(|| {
                        containers
                            .selected_group_key
                            .borrow()
                            .as_deref()
                            .and_then(|selected| {
                                containers.rows.borrow().iter().find_map(|row| match row {
                                    NativeContainerRow::Group(group) if group.key == selected => {
                                        Some(row.clone())
                                    }
                                    _ => None,
                                })
                            })
                    });
                if let Some(row) = selected_row {
                    self.display_container_row(row, false);
                } else {
                    containers
                        .title
                        .setStringValue(&NSString::from_str("Containers"));
                    containers.subtitle.setStringValue(&NSString::new());
                    containers.details_scroll.setHidden(true);
                    containers.inspect_code.setHidden(true);
                    containers.inspect.setHidden(true);
                    containers
                        .empty
                        .setStringValue(&NSString::from_str(if empty {
                            "No containers."
                        } else {
                            "Select a container or Compose project."
                        }));
                    containers.empty.setHidden(false);
                }
                log::info!(
                    "native container inventory applied workspace={} rows={}",
                    workspace_id,
                    containers.rows.borrow().len()
                );
                self.update_container_actions();
            }
            Err(error) => {
                self.show_containers_error(&error);
                log::warn!("native container inventory failed workspace={workspace_id}: {error}");
            }
        }
        self.complete_pending_page_service("containers", page_service_result);
    }

    fn show_containers_error(&self, error: &str) {
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        containers.loading.set(false);
        containers.dirty.set(false);
        containers.action_in_progress.set(false);
        self.set_page_badge("containers", NativePageBadge::None);
        containers.rows.borrow_mut().clear();
        containers.expanded_groups.borrow_mut().clear();
        containers.selected_id.borrow_mut().take();
        containers.selected_group_key.borrow_mut().take();
        containers.table.reloadData();
        containers.scroll.setHidden(true);
        unsafe { containers.spinner.stopAnimation(None) };
        containers.spinner.setHidden(true);
        containers.status.setHidden(false);
        containers
            .status
            .setStringValue(&NSString::from_str("Containers unavailable."));
        containers
            .status
            .setToolTip(Some(&NSString::from_str(error)));
        containers
            .title
            .setStringValue(&NSString::from_str("Containers Error"));
        containers.subtitle.setStringValue(&NSString::new());
        containers.details_scroll.setHidden(true);
        containers.inspect_code.setHidden(true);
        containers.inspect.setHidden(true);
        containers.empty.setStringValue(&NSString::from_str(error));
        containers.empty.setHidden(false);
        self.update_container_actions();
    }

}
