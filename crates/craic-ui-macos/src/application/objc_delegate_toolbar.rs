impl AppDelegate {
    fn handle_objc_delegate_table_view_height_of_row(&self, table: &NSTableView, row: isize) -> f64 {
        if self
            .ivars()
            .workspace_table
            .get()
            .is_some_and(|workspace_table| std::ptr::eq(table, &**workspace_table))
        {
            return WORKSPACE_ROW_HEIGHT;
        }
        if self
            .ivars()
            .author_table
            .get()
            .is_some_and(|author_table| std::ptr::eq(table, &**author_table))
        {
            return AUTHOR_ROW_HEIGHT;
        }
        if self
            .ivars()
            .agents
            .get()
            .is_some_and(|agents| std::ptr::eq(table, &*agents.transcript_table))
        {
            return usize::try_from(row)
                .ok()
                .and_then(|row| {
                    self.ivars()
                        .agents
                        .get()?
                        .transcript_items
                        .borrow()
                        .get(row)
                        .map(|item| {
                            native_agent_transcript_row_height(
                                item,
                                table.bounds().size.width,
                                self.ivars().font_sizes.get().agent,
                            )
                        })
                })
                .unwrap_or(64.0);
        }
        if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &*files.preview_table))
        {
            return 26.0;
        }
        if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &**files.table))
        {
            return FILE_ROW_HEIGHT;
        }
        if self
            .ivars()
            .containers
            .get()
            .is_some_and(|containers| std::ptr::eq(table, &**containers.table))
        {
            return CONTAINER_ROW_HEIGHT;
        }
        let Some(history) = self.ivars().history.get() else {
            return table.rowHeight();
        };
        if std::ptr::eq(table, &*history.table)
            && row >= 0
            && row as usize == history.commits.borrow().len()
            && history.loading.get()
        {
            HISTORY_LOADING_ROW_HEIGHT
        } else {
            table.rowHeight()
        }
    }

    fn handle_objc_delegate_toolbar_default_item_identifiers(
        &self,
        _toolbar: &NSToolbar,
    ) -> Retained<NSArray<NSToolbarItemIdentifier>> {
        self.toolbar_item_identifiers()
    }

    fn handle_objc_delegate_toolbar_allowed_item_identifiers(
        &self,
        _toolbar: &NSToolbar,
    ) -> Retained<NSArray<NSToolbarItemIdentifier>> {
        self.toolbar_item_identifiers()
    }

    fn handle_objc_delegate_toolbar_item(
        &self,
        _toolbar: &NSToolbar,
        identifier: &NSToolbarItemIdentifier,
        will_be_inserted: bool,
    ) -> Option<Retained<NSToolbarItem>> {
        self.make_toolbar_item(identifier, will_be_inserted)
    }
}
