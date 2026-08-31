use super::table_view::TableView;
use adw::prelude::*;
use craic_ui_preview::csv_table::parse_csv_table;
use std::rc::Rc;

pub struct CsvPreview {
    pub root: gtk::Box,
    status_label: gtk::Label,
    table_view: TableView,
}

impl CsvPreview {
    pub fn new() -> Rc<Self> {
        let status_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label"])
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(10)
            .margin_end(10)
            .build();
        let table_view = TableView::new("No CSV data to display.");
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&status_label);
        root.append(&table_view.root);

        Rc::new(Self {
            root,
            status_label,
            table_view,
        })
    }

    pub fn set_source(&self, source: &str) {
        match parse_csv_table(source) {
            Ok(Some(table)) => {
                let visible_rows = table.rows.len();
                if table.total_rows > visible_rows {
                    self.status_label.set_text(&format!(
                        "{} columns · showing first {} of {} rows",
                        table.columns.len(),
                        visible_rows,
                        table.total_rows
                    ));
                } else {
                    self.status_label.set_text(&format!(
                        "{} columns · {} rows",
                        table.columns.len(),
                        table.total_rows
                    ));
                }
                self.table_view.set_data(table.columns, table.rows);
            }
            Ok(None) => {
                self.status_label.set_text("CSV table");
                self.table_view.show_message("This CSV file is empty.");
            }
            Err(message) => {
                log::warn!("csv table preview parse failed: {message}");
                self.status_label.set_text("Unable to display CSV table");
                self.table_view.show_message(&message);
            }
        }
    }

    pub fn clear(&self) {
        self.status_label.set_text("");
        self.table_view.show_message("No CSV data to display.");
    }
}
