impl AppDelegate {
    fn clear_csv_table_preview(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.preview_table_scroll.setHidden(true);
        files.preview_table_rows.borrow_mut().clear();
        files.preview_table_columns.borrow_mut().clear();
        // NSTableView exposes a live NSArray here. Snapshot its retained members before removing
        // them; mutating the table while objc2 is enumerating that array aborts the process.
        let columns = files
            .preview_table
            .tableColumns()
            .iter()
            .collect::<Vec<_>>();
        for column in columns {
            files.preview_table.removeTableColumn(&column);
        }
        files.preview_table.reloadData();
    }

    fn clear_sqlite_preview(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files
            .sqlite_generation
            .set(files.sqlite_generation.get().wrapping_add(1));
        files.sqlite_state.borrow_mut().take();
        files.sqlite_controls.setHidden(true);
        files.sqlite_table_selector.removeAllItems();
        files.sqlite_column_selector.removeAllItems();
        files.sqlite_filter.setStringValue(&NSString::new());
        files.sqlite_previous.setEnabled(false);
        files.sqlite_next.setEnabled(false);
        files.sqlite_reload.setEnabled(false);
        files
            .sqlite_status
            .setStringValue(&NSString::from_str("Page –"));
    }

    fn request_workspace_sqlite_page(&self) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(path) = files.selected_path.borrow().clone() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let Some((db_path, table, page, filter_column, filter, sort)) =
            files.sqlite_state.borrow().as_ref().and_then(|state| {
                state
                    .tables
                    .get(state.selected_table)
                    .cloned()
                    .map(|table| {
                        (
                            state.db_path.clone(),
                            table,
                            state.page,
                            state.filter_column,
                            state.filter.clone(),
                            state.sort.clone(),
                        )
                    })
            })
        else {
            return;
        };
        let generation = files.sqlite_generation.get().wrapping_add(1);
        files.sqlite_generation.set(generation);
        files.sqlite_previous.setEnabled(false);
        files.sqlite_next.setEnabled(false);
        files
            .sqlite_status
            .setStringValue(&NSString::from_str("Loading…"));
        files.preview_spinner.setHidden(false);
        unsafe { files.preview_spinner.startAnimation(None) };
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadWorkspaceSqlitePage {
            workspace_id,
            path,
            db_path,
            table,
            page,
            filter_column,
            filter,
            sort,
            generation,
            cancellation,
        }) {
            unsafe { files.preview_spinner.stopAnimation(None) };
            files.preview_spinner.setHidden(true);
            files
                .sqlite_status
                .setStringValue(&NSString::from_str("Load failed"));
            files.empty.setStringValue(&NSString::from_str(&format!(
                "Unable to queue SQLite rows: {error}"
            )));
            files.empty.setHidden(false);
        }
    }

    fn apply_workspace_sqlite_schema(
        &self,
        workspace_id: &str,
        path: &FileNodePath,
        request_id: u64,
        result: Result<NativeSqliteSchema, String>,
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
                "discarding stale SQLite schema workspace={workspace_id} path={} request_id={request_id}",
                path.display()
            );
            return;
        }
        unsafe { files.preview_spinner.stopAnimation(None) };
        files.preview_spinner.setHidden(true);
        files.preview_scroll.setHidden(true);
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
            Ok(schema) if schema.tables.is_empty() => {
                files
                    .empty
                    .setStringValue(&NSString::from_str("This database has no tables or views."));
                files.empty.setHidden(false);
            }
            Ok(schema) => {
                for table in &schema.tables {
                    let suffix = if table.kind == "view" { " (view)" } else { "" };
                    files
                        .sqlite_table_selector
                        .addItemWithTitle(&NSString::from_str(&format!("{}{suffix}", table.name)));
                }
                files
                    .sqlite_column_selector
                    .addItemWithTitle(&NSString::from_str("All columns"));
                files.sqlite_state.replace(Some(NativeSqliteState {
                    db_path: schema.db_path,
                    _materialized: schema.materialized,
                    tables: schema.tables,
                    selected_table: 0,
                    columns: Vec::new(),
                    page: 0,
                    total_rows: 0,
                    filter: String::new(),
                    filter_column: None,
                    sort: None,
                }));
                files.sqlite_controls.setHidden(false);
                files.sqlite_reload.setEnabled(true);
                files.empty.setHidden(true);
                self.layout_files_preview();
                self.request_workspace_sqlite_page();
            }
            Err(error) => {
                files.empty.setStringValue(&NSString::from_str(&format!(
                    "Unable to preview SQLite database: {error}"
                )));
                files.empty.setHidden(false);
                log::warn!(
                    "native SQLite schema failed workspace={workspace_id} path={}: {error}",
                    path.display()
                );
            }
        }
    }

    fn apply_workspace_sqlite_page(
        &self,
        workspace_id: &str,
        path: &FileNodePath,
        generation: u64,
        result: Result<sqlite_preview::Page, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.sqlite_generation.get() != generation
            || files.selected_path.borrow().as_ref() != Some(path)
        {
            return;
        }
        unsafe { files.preview_spinner.stopAnimation(None) };
        files.preview_spinner.setHidden(true);
        self.clear_csv_table_preview();
        files
            .preview_table_scroll
            .setBorderType(NSBorderType::BezelBorder);
        match result {
            Ok(page) => {
                let selected_table_matches = files
                    .sqlite_state
                    .borrow()
                    .as_ref()
                    .and_then(|state| state.tables.get(state.selected_table))
                    == Some(&page.table);
                if !selected_table_matches {
                    return;
                }
                if let Some(state) = files.sqlite_state.borrow_mut().as_mut() {
                    state.columns = page.columns.clone();
                    state.page = page.page;
                    state.total_rows = page.total_rows;
                    if state
                        .filter_column
                        .is_some_and(|index| index >= state.columns.len())
                    {
                        state.filter_column = None;
                    }
                }
                let (selected_filter_column, active_sort) = files
                    .sqlite_state
                    .borrow()
                    .as_ref()
                    .map(|state| (state.filter_column, state.sort.clone()))
                    .unwrap_or((None, None));
                for (index, column) in page.columns.iter().enumerate() {
                    let table_column = NSTableColumn::initWithIdentifier(
                        NSTableColumn::alloc(self.mtm()),
                        &NSUserInterfaceItemIdentifier::from_str(&format!("sqlite.{index}")),
                    );
                    table_column.setTitle(&NSString::from_str(&column.name));
                    table_column.setWidth(160.0);
                    table_column.setMinWidth(80.0);
                    table_column.setMaxWidth(420.0);
                    files.preview_table.addTableColumn(&table_column);
                    if let Some(sort) = active_sort.as_ref()
                        && sort.column_index == index
                    {
                        let symbol = match sort.direction {
                            NativeSqliteSortDirection::Ascending => "chevron.up",
                            NativeSqliteSortDirection::Descending => "chevron.down",
                        };
                        let indicator = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                            &NSString::from_str(symbol),
                            Some(&NSString::from_str("SQLite sort direction")),
                        );
                        files
                            .preview_table
                            .setIndicatorImage_inTableColumn(indicator.as_deref(), &table_column);
                    }
                }
                files.preview_table_columns.replace(
                    page.columns
                        .iter()
                        .map(|column| column.name.clone())
                        .collect(),
                );
                files.preview_table_rows.replace(page.rows);
                let content_size = files.preview_table_scroll.contentSize();
                files.preview_table.setFrameSize(NSSize::new(
                    (files.preview_table_columns.borrow().len() as f64 * 160.0)
                        .max(content_size.width),
                    (files.preview_table_rows.borrow().len() as f64 * 27.0)
                        .max(content_size.height),
                ));
                files.preview_table.reloadData();
                files.sqlite_column_selector.removeAllItems();
                files
                    .sqlite_column_selector
                    .addItemWithTitle(&NSString::from_str("All columns"));
                for column in &page.columns {
                    files
                        .sqlite_column_selector
                        .addItemWithTitle(&NSString::from_str(&column.name));
                }
                files
                    .sqlite_column_selector
                    .selectItemAtIndex(
                        selected_filter_column.map_or(0, |index| index + 1) as isize,
                    );
                let first = if page.total_rows == 0 {
                    0
                } else {
                    page.page * sqlite_preview::PAGE_SIZE + 1
                };
                let last =
                    page.page * sqlite_preview::PAGE_SIZE + files.preview_table_rows.borrow().len();
                files
                    .sqlite_status
                    .setStringValue(&NSString::from_str(&format!(
                        "{first}–{last} of {}",
                        page.total_rows
                    )));
                files.sqlite_previous.setEnabled(page.page > 0);
                files.sqlite_next.setEnabled(last < page.total_rows);
                files.preview_table_scroll.setHidden(false);
                files.empty.setHidden(true);
                self.set_workspace_file_editor_status(Some(&format!(
                    "{} · rows {first}–{last} of {}",
                    page.table.name, page.total_rows
                )));
                self.layout_files_preview();
                log::debug!(
                    "native SQLite rows applied path={} table={} page={} rows={} total={}",
                    path.display(),
                    page.table.name,
                    page.page + 1,
                    files.preview_table_rows.borrow().len(),
                    page.total_rows
                );
            }
            Err(error) => {
                files.sqlite_previous.setEnabled(false);
                files.sqlite_next.setEnabled(false);
                files
                    .sqlite_status
                    .setStringValue(&NSString::from_str("Load failed"));
                files.empty.setStringValue(&NSString::from_str(&format!(
                    "Unable to load SQLite rows: {error}"
                )));
                files.empty.setHidden(false);
                log::warn!("native SQLite rows failed path={}: {error}", path.display());
            }
        }
    }

    fn apply_csv_table_preview(&self, result: Result<Option<CsvTable>, String>) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        self.clear_csv_table_preview();
        files
            .preview_table_scroll
            .setBorderType(NSBorderType::BezelBorder);
        files.preview_scroll.setHidden(true);
        files.preview_web_mode.set(NativeWebPreviewMode::Hidden);
        files.preview_web.setHidden(true);
        files.preview_divider.setHidden(true);
        match result {
            Ok(Some(table)) => {
                let visible_rows = table.rows.len();
                let status = if table.total_rows > visible_rows {
                    format!(
                        "{} columns · showing first {} of {} rows",
                        table.columns.len(),
                        visible_rows,
                        table.total_rows
                    )
                } else {
                    format!(
                        "{} columns · {} rows",
                        table.columns.len(),
                        table.total_rows
                    )
                };
                for (index, title) in table.columns.iter().enumerate() {
                    let column = NSTableColumn::initWithIdentifier(
                        NSTableColumn::alloc(self.mtm()),
                        &NSUserInterfaceItemIdentifier::from_str(&format!("csv.{index}")),
                    );
                    column.setTitle(&NSString::from_str(title));
                    column.setWidth(160.0);
                    column.setMinWidth(80.0);
                    column.setMaxWidth(360.0);
                    files.preview_table.addTableColumn(&column);
                }
                files.preview_table_columns.replace(table.columns);
                files.preview_table_rows.replace(table.rows);
                let content_size = files.preview_table_scroll.contentSize();
                files.preview_table.setFrameSize(NSSize::new(
                    (files.preview_table_columns.borrow().len() as f64 * 160.0)
                        .max(content_size.width),
                    (files.preview_table_rows.borrow().len() as f64 * 27.0)
                        .max(content_size.height),
                ));
                files.preview_table.reloadData();
                files.preview_table_scroll.setHidden(false);
                files.empty.setHidden(true);
                self.set_workspace_file_editor_status(Some(&status));
            }
            Ok(None) => {
                files
                    .empty
                    .setStringValue(&NSString::from_str("This CSV file is empty."));
                files.empty.setHidden(false);
            }
            Err(error) => {
                files.empty.setStringValue(&NSString::from_str(&error));
                files.empty.setHidden(false);
                log::warn!("native CSV table preview parse failed: {error}");
            }
        }
        self.layout_files_preview();
    }

    fn toggle_file_tree_row(&self, row: usize) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(row_data) = self.filtered_file_tree_rows().get(row).cloned() else {
            return;
        };
        if !row_data.info.capabilities.listable {
            return;
        }
        let mut expanded = files.expanded.borrow_mut();
        if !expanded.remove(&row_data.info.path) {
            expanded.insert(row_data.info.path);
        }
        drop(expanded);
        self.request_files_tree();
    }

    fn request_files_tree(&self) {
        let Some(files) = self.ivars().files.get() else {
            self.complete_pending_page_service(
                "files",
                Err("The Files page is unavailable".to_string()),
            );
            return;
        };
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            self.complete_pending_page_service("files", Err("No workspace is active".to_string()));
            return;
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            self.complete_pending_page_service(
                "files",
                Err("The workspace is not loaded".to_string()),
            );
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            self.complete_pending_page_service(
                "files",
                Err("The workspace is shutting down".to_string()),
            );
            return;
        };
        if files.rows.borrow().is_empty()
            && let Some(path) = self.ivars().pending_files_path.borrow().clone()
        {
            let root = handle.workspace_files().root();
            let target = root.join_child(path);
            files.selected_path.replace(Some(target.clone()));
            let mut parent = target.parent();
            while let Some(path) = parent {
                if path.is_root() {
                    break;
                }
                files.expanded.borrow_mut().insert(path.clone());
                parent = path.parent();
            }
        }
        let generation = files.generation.get().wrapping_add(1);
        files.generation.set(generation);
        files.loading.set(true);
        self.set_page_badge("files", NativePageBadge::Indicator);
        files.dirty.set(false);
        files.status.setHidden(false);
        files
            .status
            .setStringValue(&NSString::from_str("Loading workspace files…"));
        files.spinner.setHidden(false);
        unsafe { files.spinner.startAnimation(None) };
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.complete_pending_page_service(
                "files",
                Err("The repository service is unavailable".to_string()),
            );
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::LoadFilesTree {
            workspace_id,
            handle,
            expanded: files.expanded.borrow().clone(),
            generation,
            cancellation,
        }) {
            files.loading.set(false);
            unsafe { files.spinner.stopAnimation(None) };
            files.spinner.setHidden(true);
            files.status.setStringValue(&NSString::from_str(&format!(
                "Unable to queue file loading: {error}"
            )));
            self.set_page_badge("files", NativePageBadge::None);
            self.complete_pending_page_service("files", Err(error.to_string()));
        }
    }

    fn subscribe_files_monitor(&self, workspace_id: &str) {
        self.ivars().files_monitor.borrow_mut().take();
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            return;
        };
        let Some(completions) = self.ivars().frontend_completions.get().cloned() else {
            return;
        };
        let access = handle.workspace_files();
        let mut watched = HashSet::from([access.root()]);
        watched.extend(files.expanded.borrow().iter().cloned());
        if let Some(selected) = files.selected_path.borrow().clone() {
            watched.insert(selected);
        }
        let watched_workspace = workspace_id.to_string();
        let callback_workspace = watched_workspace.clone();
        match access.watch(FileWatchRequest {
            paths: watched.into_iter().collect(),
        }) {
            Ok((subscription, receiver)) => {
                self.ivars()
                    .files_monitor
                    .replace(Some(NativeFileMonitor::new(
                        subscription,
                        receiver,
                        callback_workspace,
                        completions,
                    )));
                log::debug!("native Files watch subscribed workspace={watched_workspace}");
            }
            Err(error) => {
                log::warn!("native Files watch unavailable workspace={watched_workspace}: {error}");
            }
        }
    }

    fn workspace_files_changed(&self, workspace_id: &str) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files.dirty.set(true);
        if self.is_active_page("files") && !files.loading.get() {
            log::debug!("native Files watch refreshing workspace={workspace_id}");
            self.request_files_tree();
        }
    }

    fn apply_files_tree(
        &self,
        workspace_id: &str,
        generation: u64,
        result: Result<Vec<NativeFileRow>, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if files.generation.get() != generation {
            return;
        }
        let page_service_result = result
            .as_ref()
            .map(|rows| serde_json::json!({ "rows": rows.len() }))
            .map_err(Clone::clone);
        files.loading.set(false);
        self.set_page_badge("files", NativePageBadge::None);
        unsafe { files.spinner.stopAnimation(None) };
        files.spinner.setHidden(true);
        match result {
            Ok(rows) => {
                files.rows.replace(rows);
                files.status.setHidden(!files.rows.borrow().is_empty());
                if files.rows.borrow().is_empty() {
                    files
                        .status
                        .setStringValue(&NSString::from_str("This workspace is empty."));
                }
            }
            Err(error) => {
                files.rows.borrow_mut().clear();
                files.status.setHidden(false);
                files.status.setStringValue(&NSString::from_str(&error));
                log::warn!("native Files tree failed workspace={workspace_id}: {error}");
            }
        }
        files.table.reloadData();
        // AppKit delivers the selection callback synchronously from selectRowIndexes:. Copy the
        // retained path before selecting so that callback can update selected_path re-entrantly.
        let selected_path = files.selected_path.borrow().clone();
        if let Some(selected) = selected_path
            && let Some(row) = self
                .filtered_file_tree_rows()
                .iter()
                .position(|candidate| candidate.info.path == selected)
        {
            let selection_changed = files.table.selectedRow() != row as isize;
            files
                .table
                .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
            files.table.scrollRowToVisible(row as isize);
            if !selection_changed {
                self.select_file_tree_row(row);
            }
        }
        self.subscribe_files_monitor(workspace_id);
        if files.dirty.get() && !files.loading.get() {
            self.request_files_tree();
        }
        self.complete_pending_page_service("files", page_service_result);
    }

}
