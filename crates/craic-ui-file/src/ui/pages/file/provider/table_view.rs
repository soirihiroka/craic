use adw::prelude::*;
use gtk::{gio, glib};

#[derive(Clone, Debug)]
pub(super) struct TableViewRow {
    pub cells: Vec<String>,
    pub is_filter: bool,
}

#[derive(Clone)]
pub(super) struct TableView {
    pub root: gtk::Stack,
    column_view: gtk::ColumnView,
    row_model: gio::ListStore,
    message_label: gtk::Label,
}

impl TableView {
    pub fn new(initial_message: &str) -> Self {
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

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&column_view)
            .build();

        let message_label = gtk::Label::builder()
            .label(initial_message)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .margin_top(16)
            .margin_start(10)
            .margin_end(10)
            .build();
        let message_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        message_box.append(&message_label);

        let root = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        root.add_named(&scroller, Some("rows"));
        root.add_named(&message_box, Some("message"));
        root.set_visible_child_name("message");

        Self {
            root,
            column_view,
            row_model,
            message_label,
        }
    }

    pub fn column_view(&self) -> &gtk::ColumnView {
        &self.column_view
    }

    pub fn reset(&self) {
        while let Some(column) = self
            .column_view
            .columns()
            .item(0)
            .and_then(|item| item.downcast::<gtk::ColumnViewColumn>().ok())
        {
            self.column_view.remove_column(&column);
        }
        self.row_model.remove_all();
    }

    pub fn append_row(&self, row: TableViewRow) {
        self.row_model.append(&glib::BoxedAnyObject::new(row));
    }

    pub fn show_rows(&self) {
        self.root.set_visible_child_name("rows");
    }

    pub fn show_message(&self, message: &str) {
        self.reset();
        self.message_label.set_text(message);
        self.root.set_visible_child_name("message");
    }

    pub fn set_data(&self, columns: Vec<String>, rows: Vec<Vec<String>>) {
        self.reset();
        if columns.is_empty() {
            self.show_message("This table does not have any columns.");
            return;
        }

        for (column_index, title) in columns.iter().enumerate() {
            let view_column =
                gtk::ColumnViewColumn::new(Some(title), Some(value_column_factory(column_index)));
            view_column.set_resizable(true);
            view_column.set_expand(false);
            view_column.set_fixed_width(initial_column_width(title, &rows, column_index));
            self.column_view.append_column(&view_column);
        }

        for cells in rows {
            self.append_row(TableViewRow {
                cells,
                is_filter: false,
            });
        }
        self.show_rows();
    }
}

fn value_column_factory(column_index: usize) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row_object) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            item.set_child(None::<&gtk::Widget>);
            return;
        };
        let row = row_object.borrow::<TableViewRow>();
        let value = row.cells.get(column_index).map_or("", String::as_str);
        item.set_child(Some(&value_cell(value)));
    });
    factory
}

pub(super) fn value_cell(value: &str) -> gtk::Widget {
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

pub(super) fn initial_column_width(title: &str, rows: &[Vec<String>], column_index: usize) -> i32 {
    let header_width = text_width_hint(title);
    let row_width = rows
        .iter()
        .take(20)
        .filter_map(|row| row.get(column_index))
        .map(|value| text_width_hint(value))
        .max()
        .unwrap_or(0);
    (header_width.max(row_width) + 36).clamp(140, 320)
}

pub(super) fn string_sorter(column_index: usize) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |left, right| {
        let left = table_row_cell(left, column_index);
        let right = table_row_cell(right, column_index);
        match left.cmp(&right) {
            std::cmp::Ordering::Less => gtk::Ordering::Smaller,
            std::cmp::Ordering::Equal => gtk::Ordering::Equal,
            std::cmp::Ordering::Greater => gtk::Ordering::Larger,
        }
    })
}

fn table_row_cell(object: &glib::Object, column_index: usize) -> String {
    object
        .downcast_ref::<glib::BoxedAnyObject>()
        .and_then(|object| {
            object
                .borrow::<TableViewRow>()
                .cells
                .get(column_index)
                .cloned()
        })
        .unwrap_or_default()
}

fn text_width_hint(text: &str) -> i32 {
    text.chars().take(36).count() as i32 * 8
}
