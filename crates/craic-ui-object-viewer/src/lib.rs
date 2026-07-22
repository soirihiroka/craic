use craic_dynamic_data::{DocumentKind, DynamicDocument, DynamicEntry, DynamicValue, ParseError};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
enum ItemLabel {
    Root(DocumentKind),
    Index(usize),
    Key(Arc<DynamicValue>),
}

#[derive(Clone)]
struct TreeItem {
    label: ItemLabel,
    value: Arc<DynamicValue>,
}

pub struct ObjectViewer {
    pub root: gtk::Stack,
    list: gtk::ListView,
    message_label: gtk::Label,
    model: RefCell<Option<gtk::TreeListModel>>,
}

impl ObjectViewer {
    pub fn new() -> Rc<Self> {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let label = gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .selectable(true)
                .margin_top(4)
                .margin_bottom(4)
                .margin_end(10)
                .build();
            let expander = gtk::TreeExpander::builder()
                .indent_for_depth(true)
                .indent_for_icon(true)
                .child(&label)
                .build();
            item.set_child(Some(&expander));
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else {
                return;
            };
            let Some(tree_item) = row.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else {
                return;
            };
            let Some(label) = expander.child().and_downcast::<gtk::Label>() else {
                return;
            };
            let tree_item = tree_item.borrow::<TreeItem>();
            let text = row_text(&tree_item);
            label.set_text(&text);
            label.set_tooltip_text(Some(&text));
            expander.set_list_row(Some(&row));
        });
        factory.connect_unbind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else {
                return;
            };
            expander.set_list_row(None);
            if let Some(label) = expander.child().and_downcast::<gtk::Label>() {
                label.set_text("");
                label.set_tooltip_text(None);
            }
        });

        let selection = gtk::NoSelection::new(None::<gio::ListModel>);
        let list = gtk::ListView::new(Some(selection), Some(factory));
        list.set_hexpand(true);
        list.set_vexpand(true);
        list.add_css_class("navigation-sidebar");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&list)
            .build();

        let message_label = gtk::Label::builder()
            .label("No structured data to display.")
            .xalign(0.0)
            .yalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .css_classes(["dim-label"])
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();
        let message_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        message_box.append(&message_label);

        let root = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        root.add_named(&scroller, Some("tree"));
        root.add_named(&message_box, Some("message"));
        root.set_visible_child_name("message");

        Rc::new(Self {
            root,
            list,
            message_label,
            model: RefCell::new(None),
        })
    }

    pub fn set_document(&self, document: DynamicDocument) {
        let root_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        root_store.append(&glib::BoxedAnyObject::new(TreeItem {
            label: ItemLabel::Root(document.kind),
            value: Arc::clone(&document.root),
        }));
        let model = gtk::TreeListModel::new(root_store, false, false, |object| {
            let item = object.downcast_ref::<glib::BoxedAnyObject>()?;
            child_model(&item.borrow::<TreeItem>().value)
        });
        let selection = gtk::NoSelection::new(Some(model.clone()));
        self.list.set_model(Some(&selection));
        if let Some(root) = model.row(0) {
            root.set_expanded(true);
        }
        self.model.replace(Some(model));
        self.root.set_visible_child_name("tree");
    }

    pub fn show_error(&self, error: &ParseError) {
        self.list.set_model(None::<&gtk::NoSelection>);
        self.model.borrow_mut().take();
        self.message_label
            .set_text(&format!("Unable to display structured data.\n\n{error}"));
        self.root.set_visible_child_name("message");
    }

    pub fn clear(&self) {
        self.list.set_model(None::<&gtk::NoSelection>);
        self.model.borrow_mut().take();
        self.message_label
            .set_text("No structured data to display.");
        self.root.set_visible_child_name("message");
    }
}

fn child_model(value: &DynamicValue) -> Option<gio::ListModel> {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    match value {
        DynamicValue::Sequence(values) => {
            for (index, value) in values.iter().enumerate() {
                store.append(&glib::BoxedAnyObject::new(TreeItem {
                    label: ItemLabel::Index(index),
                    value: Arc::clone(value),
                }));
            }
        }
        DynamicValue::Mapping(entries) => {
            for DynamicEntry { key, value } in entries {
                store.append(&glib::BoxedAnyObject::new(TreeItem {
                    label: ItemLabel::Key(Arc::clone(key)),
                    value: Arc::clone(value),
                }));
            }
        }
        _ => return None,
    }
    Some(store.upcast())
}

fn row_text(item: &TreeItem) -> String {
    let value = value_summary(&item.value);
    match &item.label {
        ItemLabel::Root(DocumentKind::Records) => match item.value.as_ref() {
            DynamicValue::Sequence(values) => format!("Records [{}]", values.len()),
            _ => value,
        },
        ItemLabel::Root(DocumentKind::Value) => value,
        ItemLabel::Index(index) => format!("[{index}]: {value}"),
        ItemLabel::Key(key) => {
            let key = match key.as_ref() {
                DynamicValue::String(value) => {
                    serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
                }
                value => value_summary(value),
            };
            format!("{key}: {value}")
        }
    }
}

fn value_summary(value: &DynamicValue) -> String {
    match value {
        DynamicValue::Null => "null".to_string(),
        DynamicValue::Boolean(value) => value.to_string(),
        DynamicValue::Number(value) => value.clone(),
        DynamicValue::String(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
        }
        DynamicValue::Sequence(values) => format!("Array [{}]", values.len()),
        DynamicValue::Mapping(entries) => {
            let object = entries
                .iter()
                .all(|entry| matches!(entry.key.as_ref(), DynamicValue::String(_)));
            if object {
                format!("Object {{{}}}", entries.len())
            } else {
                format!("Map {{{}}}", entries.len())
            }
        }
    }
}
