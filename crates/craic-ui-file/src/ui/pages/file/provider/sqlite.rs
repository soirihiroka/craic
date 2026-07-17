use super::{PreviewMatchRequest, PreviewRequest};
use crate::system::materialize::MaterializedFile;
use crate::ui::file_type;
use adw::prelude::*;
use gtk::{gio, glib};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, params_from_iter};
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const SQLITE_PAGE_SIZE: usize = 100;
const SQLITE_FILTER_DEBOUNCE_MS: u64 = 180;
const SQLITE_POLL_MS: u64 = 30;

pub fn show(request: PreviewRequest<'_>) {
    if let Some(local_path) = request.local_path {
        request.right.show_sqlite_preview(request.file_path);
        request
            .right
            .file_sqlite_preview
            .load_file(request.file_path, local_path);
        return;
    }

    request
        .right
        .show_provider_loading_message(request.file_path, "Materializing SQLite database...");
    let files = request.files.clone();
    let source = request.info.clone();
    let file_path = request.file_path.to_string();
    let apply_file_path = file_path.clone();
    let (sender, receiver) = mpsc::channel();
    crate::system::materialize::materialize_for_view(files, source, None, move |result| {
        let _ = sender.send(result);
    });
    super::receive_preview_load(
        request.right,
        request.load_token,
        file_path,
        receiver,
        move |right, result| match result {
            Ok(materialized) => {
                right.show_sqlite_preview(&apply_file_path);
                let local_path = materialized.path().to_path_buf();
                right.file_sqlite_preview.load_file_with_materialized(
                    &apply_file_path,
                    &local_path,
                    Some(materialized),
                );
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

pub fn show_match(request: PreviewMatchRequest<'_>) {
    show(request.into_preview_request());
}

pub fn has_sqlite_magic_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(SQLITE_MAGIC)
}

fn local_has_sqlite_magic(path: &Path) -> bool {
    let mut header = [0_u8; 16];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|_| &header == SQLITE_MAGIC)
}

#[derive(Clone)]
pub struct SqlitePreview {
    pub root: gtk::Box,
    table_filter_entry: gtk::Entry,
    table_list: gtk::ListBox,
    table_status_label: gtk::Label,
    rows_status_label: gtk::Label,
    rows_stack: gtk::Stack,
    column_view: gtk::ColumnView,
    row_model: gio::ListStore,
    rows_message_label: gtk::Label,
    previous_button: gtk::Button,
    page_label: gtk::Label,
    next_button: gtk::Button,
    reload_button: gtk::Button,
    state: Rc<SqlitePreviewState>,
}

#[derive(Default)]
struct SqlitePreviewState {
    file_path: RefCell<Option<String>>,
    db_path: RefCell<Option<PathBuf>>,
    materialized: RefCell<Option<MaterializedFile>>,
    tables: RefCell<Vec<SqliteTable>>,
    visible_tables: RefCell<Vec<SqliteTable>>,
    selected_table: RefCell<Option<SqliteTable>>,
    filters: RefCell<Vec<String>>,
    sort: RefCell<Option<SqliteSort>>,
    page: Cell<usize>,
    generation: Cell<u64>,
    loading: Cell<bool>,
    has_next_page: Cell<bool>,
    filter_source: RefCell<Option<glib::SourceId>>,
    sorter_handlers: RefCell<Option<SqliteSorterHandlers>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SqliteTable {
    name: String,
    kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SqliteColumn {
    name: String,
    data_type: String,
    primary_key: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SqliteSort {
    column_index: usize,
    direction: SqliteSortDirection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SqliteSortDirection {
    Ascending,
    Descending,
}

struct SqliteSorterHandlers {
    sorter: gtk::ColumnViewSorter,
    column_handler: glib::SignalHandlerId,
    order_handler: glib::SignalHandlerId,
}

struct SqliteRowsPage {
    table: SqliteTable,
    columns: Vec<SqliteColumn>,
    rows: Vec<Vec<String>>,
    total_rows: usize,
    page: usize,
}

#[derive(Clone, Debug)]
struct SqliteDisplayRow {
    cells: Vec<String>,
    is_filter: bool,
}

impl SqlitePreview {
    pub fn new() -> Rc<Self> {
        let table_filter_entry = gtk::Entry::builder()
            .placeholder_text("Search tables")
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let table_list = gtk::ListBox::new();
        table_list.add_css_class("navigation-sidebar");
        table_list.set_selection_mode(gtk::SelectionMode::Single);

        let table_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&table_list)
            .build();

        let table_status_label = muted_label("No database selected");
        table_status_label.set_margin_top(8);
        table_status_label.set_margin_bottom(8);
        table_status_label.set_margin_start(10);
        table_status_label.set_margin_end(10);

        let table_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(230)
            .build();
        table_panel.append(&table_filter_entry);
        table_panel.append(&table_scroller);
        table_panel.append(&table_status_label);

        let rows_status_label = muted_label("Select a table");
        rows_status_label.set_hexpand(true);
        rows_status_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let previous_button = gtk::Button::from_icon_name("go-previous-symbolic");
        previous_button.set_tooltip_text(Some("Previous page"));
        previous_button.add_css_class("flat");
        let page_label = gtk::Label::builder()
            .label("Page -")
            .width_chars(9)
            .xalign(0.5)
            .build();
        let next_button = gtk::Button::from_icon_name("go-next-symbolic");
        next_button.set_tooltip_text(Some("Next page"));
        next_button.add_css_class("flat");
        let reload_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        reload_button.set_tooltip_text(Some("Reload database"));
        reload_button.add_css_class("flat");

        let toolbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(10)
            .margin_end(10)
            .build();
        toolbar.append(&rows_status_label);
        toolbar.append(&previous_button);
        toolbar.append(&page_label);
        toolbar.append(&next_button);
        toolbar.append(&reload_button);

        let row_model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::SingleSelection::new(Some(row_model.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let column_view = gtk::ColumnView::new(Some(selection));
        column_view.set_hexpand(true);
        column_view.set_vexpand(true);
        column_view.set_show_column_separators(true);
        column_view.set_show_row_separators(true);
        column_view.set_reorderable(false);

        let rows_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&column_view)
            .build();

        let rows_message_label = muted_label("Select a SQLite database to preview.");
        rows_message_label.set_margin_top(16);
        rows_message_label.set_margin_start(10);
        rows_message_label.set_margin_end(10);
        let rows_message_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        rows_message_box.append(&rows_message_label);

        let rows_stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        rows_stack.add_named(&rows_scroller, Some("rows"));
        rows_stack.add_named(&rows_message_box, Some("message"));
        rows_stack.set_visible_child_name("message");

        let rows_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        rows_panel.append(&toolbar);
        rows_panel.append(&rows_stack);

        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_start_child(Some(&table_panel));
        split.set_resize_start_child(false);
        split.set_shrink_start_child(false);
        split.set_end_child(Some(&rows_panel));
        split.set_resize_end_child(true);
        split.set_shrink_end_child(false);
        split.set_position(230);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&split);

        let preview = Rc::new(Self {
            root,
            table_filter_entry,
            table_list,
            table_status_label,
            rows_status_label,
            rows_stack,
            column_view,
            row_model,
            rows_message_label,
            previous_button,
            page_label,
            next_button,
            reload_button,
            state: Rc::new(SqlitePreviewState::default()),
        });
        preview.connect_signals();
        preview.show_rows_message("Select a SQLite database to preview.");
        preview.update_controls();
        preview
    }

    pub fn clear(&self) {
        self.cancel_filter_debounce();
        self.disconnect_sorter_signals();
        self.next_generation();
        self.state.file_path.borrow_mut().take();
        self.state.db_path.borrow_mut().take();
        self.state.materialized.borrow_mut().take();
        self.state.tables.borrow_mut().clear();
        self.state.visible_tables.borrow_mut().clear();
        self.state.selected_table.borrow_mut().take();
        self.state.filters.borrow_mut().clear();
        self.state.sort.borrow_mut().take();
        self.state.page.set(0);
        self.state.loading.set(false);
        self.state.has_next_page.set(false);
        if !self.table_filter_entry.text().is_empty() {
            self.table_filter_entry.set_text("");
        }
        clear_list_box(&self.table_list);
        self.table_status_label.set_text("No database selected");
        self.rows_status_label.set_text("Select a table");
        self.show_rows_message("Select a SQLite database to preview.");
        self.update_controls();
    }

    pub fn load_file(&self, file_path: &str, db_path: &Path) {
        self.load_file_with_materialized(file_path, db_path, None);
    }

    pub fn load_file_with_materialized(
        &self,
        file_path: &str,
        db_path: &Path,
        materialized: Option<MaterializedFile>,
    ) {
        let detected_by = if local_has_sqlite_magic(db_path) {
            "magic"
        } else if file_type::is_sqlite_database_name(file_path) {
            "extension"
        } else {
            "provider"
        };
        log::info!("sqlite preview load file_path={file_path} detected_by={detected_by}");

        self.cancel_filter_debounce();
        self.disconnect_sorter_signals();
        let generation = self.next_generation();
        self.state.file_path.replace(Some(file_path.to_string()));
        self.state.db_path.replace(Some(db_path.to_path_buf()));
        self.state.materialized.replace(materialized);
        self.state.tables.borrow_mut().clear();
        self.state.visible_tables.borrow_mut().clear();
        self.state.selected_table.borrow_mut().take();
        self.state.filters.borrow_mut().clear();
        self.state.sort.borrow_mut().take();
        self.state.page.set(0);
        self.state.loading.set(true);
        self.state.has_next_page.set(false);
        if !self.table_filter_entry.text().is_empty() {
            self.table_filter_entry.set_text("");
        }
        clear_list_box(&self.table_list);
        self.table_status_label.set_text("Loading tables...");
        self.rows_status_label.set_text("Loading database...");
        self.show_rows_message("Loading database...");
        self.update_controls();

        let db_path = db_path.to_path_buf();
        self.spawn_load(
            generation,
            move || load_schema(&db_path),
            move |preview, result| preview.finish_schema_load(result),
        );
    }

    fn connect_signals(self: &Rc<Self>) {
        self.table_filter_entry.connect_changed({
            let preview = self.clone();

            move |entry| preview.populate_table_list(&entry.text())
        });

        self.table_list.connect_row_selected({
            let preview = self.clone();

            move |_, row| {
                let Some(row) = row else {
                    return;
                };
                let index = row.index();
                if index < 0 {
                    return;
                }
                preview.select_visible_table(index as usize);
            }
        });

        self.previous_button.connect_clicked({
            let preview = self.clone();

            move |_| preview.previous_page()
        });

        self.next_button.connect_clicked({
            let preview = self.clone();

            move |_| preview.next_page()
        });

        self.reload_button.connect_clicked({
            let preview = self.clone();

            move |_| preview.reload()
        });
    }

    fn finish_schema_load(&self, result: Result<Vec<SqliteTable>, String>) {
        self.state.loading.set(false);
        match result {
            Ok(tables) => {
                log::info!("sqlite preview schema loaded tables={}", tables.len());
                self.state.tables.replace(tables);
                self.populate_table_list(&self.table_filter_entry.text());
            }
            Err(err) => {
                log::warn!("sqlite preview schema load failed: {err}");
                self.state.tables.borrow_mut().clear();
                self.state.visible_tables.borrow_mut().clear();
                self.state.selected_table.borrow_mut().take();
                self.table_status_label.set_text("Unable to load database");
                self.rows_status_label
                    .set_text("Unable to load SQLite database");
                self.show_rows_message(&format!("Unable to load SQLite database: {err}"));
            }
        }
        self.update_controls();
    }

    fn populate_table_list(&self, filter: &str) {
        clear_list_box(&self.table_list);

        let filter = filter.trim().to_ascii_lowercase();
        let tables = self.state.tables.borrow();
        let visible_tables = tables
            .iter()
            .filter(|table| {
                filter.is_empty() || table.name.to_ascii_lowercase().contains(filter.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        self.state.visible_tables.replace(visible_tables.clone());

        for table in &visible_tables {
            self.table_list.append(&table_row(table));
        }

        if tables.is_empty() {
            self.state.selected_table.borrow_mut().take();
            self.table_status_label.set_text("No tables or views");
            self.rows_status_label.set_text("No tables or views");
            self.show_rows_message("This database does not contain tables or views.");
            self.update_controls();
            return;
        }

        if visible_tables.is_empty() {
            self.table_status_label.set_text("No matching tables");
            self.update_controls();
            return;
        }

        let selected_name = self
            .state
            .selected_table
            .borrow()
            .as_ref()
            .map(|table| table.name.clone());
        let selected_index = selected_name
            .and_then(|name| visible_tables.iter().position(|table| table.name == name))
            .unwrap_or(0);
        self.table_status_label
            .set_text(&format!("{} tables/views", visible_tables.len()));
        if let Some(row) = self.table_list.row_at_index(selected_index as i32) {
            self.table_list.select_row(Some(&row));
        }
    }

    fn select_visible_table(&self, index: usize) {
        let table = self.state.visible_tables.borrow().get(index).cloned();
        let Some(table) = table else {
            return;
        };
        if self.state.selected_table.borrow().as_ref() == Some(&table) {
            return;
        }

        log::info!(
            "sqlite preview table selected table={} kind={}",
            table.name,
            table.kind
        );
        self.state.selected_table.replace(Some(table));
        self.state.filters.borrow_mut().clear();
        self.state.sort.borrow_mut().take();
        self.state.page.set(0);
        self.load_current_page();
    }

    fn previous_page(&self) {
        let page = self.state.page.get();
        if page == 0 || self.state.loading.get() {
            return;
        }
        self.state.page.set(page - 1);
        self.load_current_page();
    }

    fn next_page(&self) {
        if !self.state.has_next_page.get() || self.state.loading.get() {
            return;
        }
        self.state.page.set(self.state.page.get() + 1);
        self.load_current_page();
    }

    fn reload(&self) {
        let Some(file_path) = self.state.file_path.borrow().clone() else {
            return;
        };
        let Some(db_path) = self.state.db_path.borrow().clone() else {
            return;
        };
        log::info!("sqlite preview reload file_path={file_path}");
        let materialized = self.state.materialized.borrow_mut().take();
        self.load_file_with_materialized(&file_path, &db_path, materialized);
    }

    fn set_filter(&self, index: usize, value: String) {
        {
            let mut filters = self.state.filters.borrow_mut();
            if filters.len() <= index {
                filters.resize(index + 1, String::new());
            }
            if filters[index] == value {
                return;
            }
            filters[index] = value;
        }

        self.state.page.set(0);
        self.cancel_filter_debounce();
        log::debug!("sqlite preview filter changed column_index={index}");
        let preview = self.clone();
        let source_id = glib::timeout_add_local_once(
            Duration::from_millis(SQLITE_FILTER_DEBOUNCE_MS),
            move || {
                preview.state.filter_source.borrow_mut().take();
                preview.load_current_page();
            },
        );
        self.state.filter_source.replace(Some(source_id));
    }

    fn load_current_page(&self) {
        let Some(db_path) = self.state.db_path.borrow().clone() else {
            return;
        };
        let Some(table) = self.state.selected_table.borrow().clone() else {
            return;
        };
        let page = self.state.page.get();
        let filters = self.state.filters.borrow().clone();
        let sort = self.state.sort.borrow().clone();
        let generation = self.next_generation();
        self.state.loading.set(true);
        self.state.has_next_page.set(false);
        self.rows_status_label
            .set_text(&format!("Loading rows from {}...", table.name));
        self.show_rows_message("Loading rows...");
        self.update_controls();

        log::debug!(
            "sqlite preview rows load table={} page={} filters={} sort={:?}",
            table.name,
            page + 1,
            filters.iter().filter(|filter| !filter.is_empty()).count(),
            sort
        );
        self.spawn_load(
            generation,
            move || load_rows(&db_path, table, page, filters, sort),
            move |preview, result| preview.finish_rows_load(result),
        );
    }

    fn finish_rows_load(&self, result: Result<SqliteRowsPage, String>) {
        self.state.loading.set(false);
        match result {
            Ok(page) => {
                let has_next_page = (page.page + 1) * SQLITE_PAGE_SIZE < page.total_rows;
                self.state.has_next_page.set(has_next_page);
                self.state.page.set(page.page);
                self.ensure_filter_len(page.columns.len());
                self.render_rows_page(&page);
                log::debug!(
                    "sqlite preview rows loaded table={} page={} rows={} total={}",
                    page.table.name,
                    page.page + 1,
                    page.rows.len(),
                    page.total_rows
                );
            }
            Err(err) => {
                log::warn!("sqlite preview row load failed: {err}");
                self.state.has_next_page.set(false);
                self.rows_status_label.set_text("Unable to load rows");
                self.show_rows_message(&format!("Unable to load rows: {err}"));
            }
        }
        self.update_controls();
    }

    fn render_rows_page(&self, page: &SqliteRowsPage) {
        self.disconnect_sorter_signals();
        clear_column_view(&self.column_view);
        self.row_model.remove_all();
        if page.columns.is_empty() {
            self.rows_status_label
                .set_text(&format!("{} has no columns", page.table.name));
            self.show_rows_message("This table does not have visible columns.");
            return;
        }

        let filters = self.state.filters.borrow().clone();
        let sort = self.state.sort.borrow().clone();
        let mut active_sort_column = None;
        for (column_index, column) in page.columns.iter().enumerate() {
            let view_column = gtk::ColumnViewColumn::new(
                Some(&column.name),
                Some(column_factory(self, column_index, column.clone())),
            );
            view_column.set_id(Some(&column_index.to_string()));
            view_column.set_resizable(true);
            view_column.set_expand(false);
            view_column.set_fixed_width(initial_column_width(page, column_index));
            view_column.set_sorter(Some(&sqlite_column_sorter(column_index)));
            if sort
                .as_ref()
                .is_some_and(|sort| sort.column_index == column_index)
            {
                active_sort_column = Some(view_column.clone());
            }
            self.column_view.append_column(&view_column);
        }
        if let (Some(sort), Some(column)) = (&sort, active_sort_column) {
            self.column_view
                .sort_by_column(Some(&column), sort.direction.into_sort_type());
        }
        self.connect_sorter_signals();

        self.row_model
            .append(&glib::BoxedAnyObject::new(SqliteDisplayRow {
                cells: filters_for_columns(&filters, page.columns.len()),
                is_filter: true,
            }));

        for row in &page.rows {
            self.row_model
                .append(&glib::BoxedAnyObject::new(SqliteDisplayRow {
                    cells: row.clone(),
                    is_filter: false,
                }));
        }

        if page.total_rows == 0 {
            self.rows_status_label
                .set_text(&format!("{}: 0 rows", page.table.name));
        } else {
            let first_row = page.page * SQLITE_PAGE_SIZE + 1;
            let last_row = page.page * SQLITE_PAGE_SIZE + page.rows.len();
            self.rows_status_label.set_text(&format!(
                "{}: rows {}-{} of {}",
                page.table.name, first_row, last_row, page.total_rows
            ));
        }
        self.rows_stack.set_visible_child_name("rows");
    }

    fn show_rows_message(&self, message: &str) {
        self.disconnect_sorter_signals();
        clear_column_view(&self.column_view);
        self.row_model.remove_all();
        self.rows_message_label.set_text(message);
        self.rows_stack.set_visible_child_name("message");
    }

    fn update_controls(&self) {
        let loading = self.state.loading.get();
        let has_selected_table = self.state.selected_table.borrow().is_some();
        let page = self.state.page.get();
        self.previous_button
            .set_sensitive(has_selected_table && !loading && page > 0);
        self.next_button
            .set_sensitive(has_selected_table && !loading && self.state.has_next_page.get());
        self.reload_button
            .set_sensitive(self.state.db_path.borrow().is_some() && !loading);
        if has_selected_table {
            self.page_label.set_text(&format!("Page {}", page + 1));
        } else {
            self.page_label.set_text("Page -");
        }
    }

    fn connect_sorter_signals(&self) {
        let Some(sorter) = self
            .column_view
            .sorter()
            .and_then(|sorter| sorter.downcast::<gtk::ColumnViewSorter>().ok())
        else {
            return;
        };

        let column_handler = sorter.connect_primary_sort_column_notify({
            let preview = self.clone();

            move |sorter| preview.sync_sort_from_column_view_sorter(sorter)
        });
        let order_handler = sorter.connect_primary_sort_order_notify({
            let preview = self.clone();

            move |sorter| preview.sync_sort_from_column_view_sorter(sorter)
        });
        self.state
            .sorter_handlers
            .replace(Some(SqliteSorterHandlers {
                sorter,
                column_handler,
                order_handler,
            }));
    }

    fn disconnect_sorter_signals(&self) {
        let Some(handlers) = self.state.sorter_handlers.borrow_mut().take() else {
            return;
        };
        handlers.sorter.disconnect(handlers.column_handler);
        handlers.sorter.disconnect(handlers.order_handler);
    }

    fn sync_sort_from_column_view_sorter(&self, sorter: &gtk::ColumnViewSorter) {
        let Some(column) = sorter.primary_sort_column() else {
            if self.state.sort.borrow_mut().take().is_some() {
                log::debug!("sqlite preview sort cleared");
                self.state.page.set(0);
                self.load_current_page();
            }
            return;
        };
        let Some(column_index) = column
            .id()
            .as_deref()
            .and_then(|id| id.parse::<usize>().ok())
        else {
            return;
        };
        let direction = SqliteSortDirection::from_sort_type(sorter.primary_sort_order());
        let next_sort = SqliteSort {
            column_index,
            direction,
        };
        {
            let mut sort = self.state.sort.borrow_mut();
            if sort.as_ref() == Some(&next_sort) {
                return;
            }
            sort.replace(next_sort.clone());
        }

        let column_name = column
            .title()
            .map(|title| title.to_string())
            .unwrap_or_else(|| column_index.to_string());
        log::info!(
            "sqlite preview sort changed column={} direction={}",
            column_name,
            next_sort.direction.label()
        );
        self.state.page.set(0);
        self.load_current_page();
    }

    fn ensure_filter_len(&self, column_count: usize) {
        let mut filters = self.state.filters.borrow_mut();
        filters.resize(column_count, String::new());
    }

    fn spawn_load<T, Work, Apply>(&self, generation: u64, work: Work, apply: Apply)
    where
        T: Send + 'static,
        Work: FnOnce() -> T + Send + 'static,
        Apply: FnMut(&SqlitePreview, T) + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = work();
            let _ = sender.send(result);
        });
        self.receive_load(generation, receiver, apply);
    }

    fn receive_load<T, Apply>(&self, generation: u64, receiver: mpsc::Receiver<T>, mut apply: Apply)
    where
        T: Send + 'static,
        Apply: FnMut(&SqlitePreview, T) + 'static,
    {
        let preview = self.clone();
        glib::timeout_add_local(
            Duration::from_millis(SQLITE_POLL_MS),
            move || match receiver.try_recv() {
                Ok(result) => {
                    if preview.state.generation.get() == generation {
                        apply(&preview, result);
                    }
                    glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    if preview.state.generation.get() == generation {
                        preview.state.loading.set(false);
                        preview.rows_status_label.set_text("Preview loading failed");
                        preview
                            .show_rows_message("SQLite preview loading did not return a result.");
                        preview.update_controls();
                    }
                    glib::ControlFlow::Break
                }
            },
        );
    }

    fn next_generation(&self) -> u64 {
        let generation = self.state.generation.get().wrapping_add(1).max(1);
        self.state.generation.set(generation);
        generation
    }

    fn cancel_filter_debounce(&self) {
        if let Some(source_id) = self.state.filter_source.borrow_mut().take() {
            source_id.remove();
        }
    }
}

fn column_factory(
    preview: &SqlitePreview,
    column_index: usize,
    column: SqliteColumn,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_bind({
        let preview = preview.clone();

        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(row_object) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                item.set_child(None::<&gtk::Widget>);
                return;
            };
            let row = row_object.borrow::<SqliteDisplayRow>();
            let value = row.cells.get(column_index).cloned().unwrap_or_default();
            let child = if row.is_filter {
                filter_cell(&preview, column_index, &column, &value)
            } else {
                value_cell(&value)
            };
            item.set_child(Some(&child));
        }
    });
    factory
}

fn filter_cell(
    preview: &SqlitePreview,
    column_index: usize,
    column: &SqliteColumn,
    value: &str,
) -> gtk::Widget {
    let metadata = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .margin_top(4)
        .margin_start(6)
        .margin_end(6)
        .build();
    metadata.append(&metadata_icon(
        type_icon_name(&column.data_type),
        &type_tooltip(&column.data_type),
    ));
    if column.primary_key {
        metadata.append(&metadata_icon("key2-symbolic", "Primary key"));
    }
    let entry = gtk::Entry::builder()
        .text(value)
        .placeholder_text("Search")
        .hexpand(true)
        .margin_bottom(4)
        .margin_start(6)
        .margin_end(6)
        .build();
    entry.connect_changed({
        let preview = preview.clone();

        move |entry| preview.set_filter(column_index, entry.text().to_string())
    });

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&metadata);
    content.append(&entry);
    content.upcast()
}

fn value_cell(value: &str) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(value)
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .selectable(true)
        .hexpand(true)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(8)
        .margin_end(8)
        .build();
    label.set_tooltip_text(Some(value));
    label.upcast()
}

fn initial_column_width(page: &SqliteRowsPage, column_index: usize) -> i32 {
    let header_width = page
        .columns
        .get(column_index)
        .map(|column| text_width_hint(&column.name))
        .unwrap_or(0);
    let row_width = page
        .rows
        .iter()
        .take(20)
        .filter_map(|row| row.get(column_index))
        .map(|value| text_width_hint(value))
        .max()
        .unwrap_or(0);
    (header_width.max(row_width) + 36).clamp(140, 320)
}

fn text_width_hint(text: &str) -> i32 {
    text.chars().take(36).count() as i32 * 8
}

fn filters_for_columns(filters: &[String], column_count: usize) -> Vec<String> {
    (0..column_count)
        .map(|index| filters.get(index).cloned().unwrap_or_default())
        .collect()
}

impl SqliteSortDirection {
    fn from_sort_type(sort_type: gtk::SortType) -> Self {
        match sort_type {
            gtk::SortType::Descending => Self::Descending,
            _ => Self::Ascending,
        }
    }

    fn into_sort_type(self) -> gtk::SortType {
        match self {
            Self::Ascending => gtk::SortType::Ascending,
            Self::Descending => gtk::SortType::Descending,
        }
    }

    fn sql(self) -> &'static str {
        match self {
            Self::Ascending => "ASC",
            Self::Descending => "DESC",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }
}

fn sqlite_column_sorter(column_index: usize) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |left, right| {
        let left = display_row_cell(left, column_index);
        let right = display_row_cell(right, column_index);
        match left.cmp(&right) {
            std::cmp::Ordering::Less => gtk::Ordering::Smaller,
            std::cmp::Ordering::Equal => gtk::Ordering::Equal,
            std::cmp::Ordering::Greater => gtk::Ordering::Larger,
        }
    })
}

fn display_row_cell(object: &glib::Object, column_index: usize) -> String {
    object
        .downcast_ref::<glib::BoxedAnyObject>()
        .and_then(|object| {
            object
                .borrow::<SqliteDisplayRow>()
                .cells
                .get(column_index)
                .cloned()
        })
        .unwrap_or_default()
}

fn metadata_icon(icon_name: &str, tooltip: &str) -> gtk::Image {
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(14);
    icon.set_tooltip_text(Some(tooltip));
    icon
}

fn type_icon_name(data_type: &str) -> &'static str {
    let normalized = data_type.trim().to_ascii_uppercase();
    if normalized.is_empty() {
        return "columns-symbolic";
    }
    if normalized.contains("INT") || normalized.contains("BOOL") {
        "lang-constant-symbolic"
    } else if normalized.contains("CHAR")
        || normalized.contains("CLOB")
        || normalized.contains("TEXT")
    {
        "completion-word-symbolic"
    } else if normalized.contains("BLOB") {
        "code-symbolic"
    } else if normalized.contains("REAL")
        || normalized.contains("FLOA")
        || normalized.contains("DOUB")
        || normalized.contains("NUM")
        || normalized.contains("DEC")
    {
        "lang-constant-symbolic"
    } else {
        "text-sql-symbolic"
    }
}

fn type_tooltip(data_type: &str) -> String {
    let data_type = data_type.trim();
    if data_type.is_empty() {
        "SQLite type: dynamic".to_string()
    } else {
        format!("SQLite type: {data_type}")
    }
}

fn load_schema(db_path: &Path) -> Result<Vec<SqliteTable>, String> {
    let conn = open_database(db_path)?;
    let mut stmt = conn
        .prepare(
            "SELECT name, type FROM sqlite_schema \
             WHERE type IN ('table', 'view') \
             ORDER BY lower(name), name",
        )
        .map_err(|err| format!("Unable to read SQLite schema: {err}"))?;
    let tables = stmt
        .query_map([], |row| {
            Ok(SqliteTable {
                name: row.get(0)?,
                kind: row.get(1)?,
            })
        })
        .map_err(|err| format!("Unable to read SQLite schema: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Unable to read SQLite schema: {err}"))?;
    Ok(tables)
}

fn load_rows(
    db_path: &Path,
    table: SqliteTable,
    page: usize,
    filters: Vec<String>,
    sort: Option<SqliteSort>,
) -> Result<SqliteRowsPage, String> {
    let conn = open_database(db_path)?;
    let table_sql = quote_identifier(&table.name);
    let columns = load_columns(&conn, &table)?;
    if columns.is_empty() {
        return Ok(SqliteRowsPage {
            table,
            columns,
            rows: Vec::new(),
            total_rows: 0,
            page,
        });
    }
    let select_sql = columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");

    let active_filters = columns
        .iter()
        .zip(filters.iter())
        .filter_map(|(column, filter)| {
            let filter = filter.trim();
            if filter.is_empty() {
                None
            } else {
                Some((column.name.as_str(), filter.to_string()))
            }
        })
        .collect::<Vec<_>>();

    let where_sql = if active_filters.is_empty() {
        String::new()
    } else {
        format!(
            " WHERE {}",
            active_filters
                .iter()
                .map(|(column, _)| {
                    format!(
                        "CAST({} AS TEXT) LIKE ? ESCAPE '\\'",
                        quote_identifier(column)
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ")
        )
    };
    let filter_patterns = active_filters
        .iter()
        .map(|(_, filter)| like_pattern(filter))
        .collect::<Vec<_>>();

    let count_sql = format!("SELECT COUNT(*) FROM {table_sql}{where_sql}");
    let total_rows = conn
        .query_row(
            &count_sql,
            params_from_iter(filter_patterns.iter()),
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count.max(0) as usize)
        .map_err(|err| format!("Unable to count rows for {}: {err}", table.name))?;

    let offset = page.saturating_mul(SQLITE_PAGE_SIZE);
    let order_sql = sort
        .as_ref()
        .and_then(|sort| {
            columns.get(sort.column_index).map(|column| {
                format!(
                    " ORDER BY {} {}",
                    quote_identifier(&column.name),
                    sort.direction.sql()
                )
            })
        })
        .unwrap_or_default();
    let rows_sql = format!(
        "SELECT {select_sql} FROM {table_sql}{where_sql}{order_sql} LIMIT {SQLITE_PAGE_SIZE} OFFSET {offset}"
    );
    let mut stmt = conn
        .prepare(&rows_sql)
        .map_err(|err| format!("Unable to read rows from {}: {err}", table.name))?;
    let mut cursor = stmt
        .query(params_from_iter(filter_patterns.iter()))
        .map_err(|err| format!("Unable to read rows from {}: {err}", table.name))?;
    let mut rows = Vec::new();
    while let Some(row) = cursor
        .next()
        .map_err(|err| format!("Unable to read row from {}: {err}", table.name))?
    {
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let value = row
                .get_ref(index)
                .map(sqlite_value_text)
                .map_err(|err| format!("Unable to read value from {}: {err}", table.name))?;
            values.push(value);
        }
        rows.push(values);
    }

    Ok(SqliteRowsPage {
        table,
        columns,
        rows,
        total_rows,
        page,
    })
}

fn load_columns(conn: &Connection, table: &SqliteTable) -> Result<Vec<SqliteColumn>, String> {
    let pragma_result = conn
        .prepare(
            "SELECT name, type, pk FROM pragma_table_xinfo(?) \
             WHERE hidden = 0 ORDER BY cid",
        )
        .and_then(|mut stmt| {
            stmt.query_map([table.name.as_str()], |row| {
                Ok(SqliteColumn {
                    name: row.get(0)?,
                    data_type: row.get(1)?,
                    primary_key: row.get::<_, i64>(2)? > 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
        });

    match pragma_result {
        Ok(columns) if !columns.is_empty() => Ok(columns),
        Ok(_) => fallback_columns(conn, table),
        Err(err) => {
            log::debug!(
                "sqlite preview pragma_table_xinfo failed table={}: {err}",
                table.name
            );
            fallback_columns(conn, table)
        }
    }
}

fn fallback_columns(conn: &Connection, table: &SqliteTable) -> Result<Vec<SqliteColumn>, String> {
    let columns_sql = format!("SELECT * FROM {} LIMIT 0", quote_identifier(&table.name));
    let columns_stmt = conn
        .prepare(&columns_sql)
        .map_err(|err| format!("Unable to read columns for {}: {err}", table.name))?;
    Ok(columns_stmt
        .column_names()
        .into_iter()
        .map(|name| SqliteColumn {
            name: name.to_string(),
            data_type: String::new(),
            primary_key: false,
        })
        .collect())
}

fn open_database(db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| format!("Unable to open SQLite database: {err}"))?;
    conn.busy_timeout(Duration::from_millis(750))
        .map_err(|err| format!("Unable to configure SQLite timeout: {err}"))?;
    conn.pragma_update(None, "query_only", true)
        .map_err(|err| format!("Unable to mark SQLite connection read-only: {err}"))?;
    Ok(conn)
}

fn sqlite_value_text(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => value.to_string(),
        ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
        ValueRef::Blob(value) => format!("<blob {} bytes>", value.len()),
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn like_pattern(filter: &str) -> String {
    let mut pattern = String::with_capacity(filter.len() + 2);
    pattern.push('%');
    for ch in filter.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn table_row(table: &SqliteTable) -> gtk::ListBoxRow {
    let icon = gtk::Image::from_icon_name(if table.kind == "view" {
        "list-compact-symbolic"
    } else {
        "text-sql-symbolic"
    });
    icon.set_pixel_size(16);

    let title = gtk::Label::builder()
        .label(&table.name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(10)
        .margin_end(10)
        .build();
    content.append(&icon);
    content.append(&title);

    let row = gtk::ListBoxRow::builder()
        .activatable(true)
        .selectable(true)
        .child(&content)
        .build();
    row.set_tooltip_text(Some(&table.name));
    row
}

fn muted_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["dim-label"])
        .build()
}

fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn clear_column_view(view: &gtk::ColumnView) {
    loop {
        let columns = view.columns();
        let Some(column) = columns
            .item(0)
            .and_then(|item| item.downcast::<gtk::ColumnViewColumn>().ok())
        else {
            break;
        };
        view.remove_column(&column);
    }
}
