use super::table_view::TableView;
use adw::prelude::*;
use std::rc::Rc;

const MAX_VISIBLE_ROWS: usize = 10_000;
const MAX_COLUMNS: usize = 256;

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

struct CsvTable {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    total_rows: usize,
}

fn parse_csv_table(source: &str) -> Result<Option<CsvTable>, String> {
    let records = parse_csv_records(source)?;
    let mut records = records.into_iter();
    let Some(header) = records.next() else {
        return Ok(None);
    };
    let rows = records.collect::<Vec<_>>();
    let column_count = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(header.len());
    if column_count > MAX_COLUMNS {
        return Err(format!(
            "This CSV has {column_count} columns; table preview supports up to {MAX_COLUMNS}."
        ));
    }

    let columns = (0..column_count)
        .map(|index| {
            header
                .get(index)
                .filter(|name| !name.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| format!("Column {}", index + 1))
        })
        .collect();
    let total_rows = rows.len();
    let rows = rows.into_iter().take(MAX_VISIBLE_ROWS).collect();
    Ok(Some(CsvTable {
        columns,
        rows,
        total_rows,
    }))
}

fn parse_csv_records(source: &str) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut field_started = false;
    let mut quoted = false;
    let mut chars = source.chars().peekable();

    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }

        match ch {
            '"' if !field_started => {
                quoted = true;
                field_started = true;
            }
            ',' => {
                record.push(std::mem::take(&mut field));
                field_started = false;
            }
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                field_started = false;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                field_started = false;
            }
            _ => {
                field.push(ch);
                field_started = true;
            }
        }
    }

    if quoted {
        return Err("The CSV contains an unterminated quoted field.".to_string());
    }
    if field_started || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}
