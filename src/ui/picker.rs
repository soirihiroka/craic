use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
    pub fallback_label: String,
    pub icon_name: Option<String>,
    pub color: Option<String>,
}

#[derive(Clone)]
pub struct Picker {
    pub button: gtk::MenuButton,
    pub add_button: gtk::Button,
    list: gtk::ListBox,
    popover: gtk::Popover,
    progress_bar: gtk::ProgressBar,
    search_entry: gtk::SearchEntry,
    button_icon_stack: gtk::Stack,
    button_icon: gtk::Image,
    button_label: gtk::Label,
    items: Rc<RefCell<Vec<PickerItem>>>,
}

impl Picker {
    pub fn new(
        placeholder: &str,
        add_tooltip: &str,
        button_label: &str,
        button_icon_name: &str,
        button_tooltip: &str,
        items: Vec<PickerItem>,
    ) -> Self {
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(placeholder)
            .hexpand(true)
            .build();
        let add_button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(add_tooltip)
            .build();
        add_button.add_css_class("flat");

        let search_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        search_row.append(&search_entry);
        search_row.append(&add_button);

        let progress_bar = gtk::ProgressBar::builder()
            .visible(false)
            .show_text(false)
            .build();

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("navigation-sidebar");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_height(120)
            .max_content_height(260)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();
        content.append(&search_row);
        content.append(&progress_bar);
        content.append(&scroller);

        let popover = gtk::Popover::builder()
            .width_request(360)
            .child(&content)
            .build();
        popover.add_css_class("menu");

        let button_icon = gtk::Image::builder()
            .icon_name(button_icon_name)
            .pixel_size(16)
            .build();
        let button_spinner = adw::Spinner::builder()
            .width_request(16)
            .height_request(16)
            .build();
        let button_icon_stack = gtk::Stack::new();
        button_icon_stack.add_named(&button_icon, Some("icon"));
        button_icon_stack.add_named(&button_spinner, Some("spinner"));
        button_icon_stack.set_visible_child_name("icon");
        let button_label = gtk::Label::builder().label(button_label).build();
        let arrow = gtk::Image::builder()
            .icon_name("pan-down-symbolic")
            .pixel_size(16)
            .build();
        let button_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        button_box.append(&button_icon_stack);
        button_box.append(&button_label);
        button_box.append(&arrow);

        let button = gtk::MenuButton::builder()
            .popover(&popover)
            .tooltip_text(button_tooltip)
            .build();
        button.add_css_class("flat");
        button.set_child(Some(&button_box));

        let picker = Self {
            button,
            add_button,
            list,
            popover,
            progress_bar,
            search_entry,
            button_icon_stack,
            button_icon,
            button_label,
            items: Rc::new(RefCell::new(items)),
        };
        picker.refresh();
        picker.connect_search();
        picker
    }

    pub fn set_button_label(&self, label: &str) {
        self.button_label.set_label(label);
    }

    pub fn set_button_icon(&self, icon_name: &str) {
        self.button_icon.set_icon_name(Some(icon_name));
        self.button_icon_stack.set_visible_child_name("icon");
    }

    pub fn set_button_spinner(&self) {
        self.button_icon_stack.set_visible_child_name("spinner");
    }

    pub fn set_items(&self, items: Vec<PickerItem>) {
        *self.items.borrow_mut() = items;
        self.refresh();
    }

    pub fn set_loading(&self, loading: bool) {
        self.progress_bar.set_visible(loading);
        if loading {
            let progress_bar = self.progress_bar.clone();
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                if !progress_bar.is_visible() {
                    return gtk::glib::ControlFlow::Break;
                }

                progress_bar.pulse();
                gtk::glib::ControlFlow::Continue
            });
        }
    }

    pub fn connect_item_activated<F: Fn(String) + 'static>(&self, callback: F) {
        self.list.connect_row_activated({
            let popover = self.button.popover();

            move |_, row| {
                let id = row.widget_name().to_string();
                if id.is_empty() {
                    return;
                }

                if let Some(popover) = popover.as_ref() {
                    popover.popdown();
                }

                callback(id);
            }
        });
    }

    pub fn connect_add_clicked<F: Fn() + 'static>(&self, callback: F) {
        self.add_button.connect_clicked({
            let popover = self.button.popover();

            move |_| {
                if let Some(popover) = popover.as_ref() {
                    popover.popdown();
                }

                callback();
            }
        });
    }

    pub fn connect_opened<F: Fn() + 'static>(&self, callback: F) {
        self.popover.connect_show(move |_| callback());
    }

    pub fn update_item_metadata(&self, id: &str, remote_label: Option<&str>, icon_name: &str) {
        let icon_name = icon_name.to_string();
        let mut label_changed = false;
        for item in self.items.borrow_mut().iter_mut() {
            if item.id == id {
                let next_label = remote_label
                    .map(ToString::to_string)
                    .unwrap_or_else(|| item.fallback_label.clone());
                if item.label != next_label {
                    item.label = next_label;
                    label_changed = true;
                }
                item.icon_name = Some(icon_name.clone());
                break;
            }
        }

        if label_changed {
            self.refresh();
            return;
        }

        self.update_visible_item_icon(id, &icon_name);
    }

    fn update_visible_item_icon(&self, id: &str, icon_name: &str) {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            let next = widget.next_sibling();

            if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
                if row.widget_name() == id {
                    if let Some(image) = find_image(row.upcast_ref()) {
                        image.set_icon_name(Some(icon_name));
                    }
                    if let Some(stack) = find_stack(row.upcast_ref()) {
                        stack.set_visible_child_name("icon");
                    }
                    break;
                }
            }

            child = next;
        }
    }

    fn connect_search(&self) {
        self.search_entry.connect_search_changed({
            let list = self.list.clone();
            let items = self.items.clone();

            move |entry| fill_items(&list, &items.borrow(), &entry.text())
        });
    }

    fn refresh(&self) {
        fill_items(&self.list, &self.items.borrow(), &self.search_entry.text());
    }
}

fn fill_items(list: &gtk::ListBox, items: &[PickerItem], filter: &str) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    let filter = filter.trim().to_lowercase();

    for item in items {
        if !filter.is_empty() && !item.label.to_lowercase().contains(&filter) {
            continue;
        }

        list.append(&item_row(item));
    }
}

fn item_row(item: &PickerItem) -> gtk::ListBoxRow {
    let icon = gtk::Image::builder()
        .pixel_size(16)
        .valign(gtk::Align::Center)
        .build();
    if let Some(icon_name) = item.icon_name.as_ref() {
        icon.set_icon_name(Some(icon_name));
    }
    let spinner = adw::Spinner::builder()
        .width_request(16)
        .height_request(16)
        .valign(gtk::Align::Center)
        .build();
    let icon_stack = gtk::Stack::new();
    icon_stack.add_named(&icon, Some("icon"));
    icon_stack.add_named(&spinner, Some("spinner"));
    icon_stack.set_visible_child_name(if item.icon_name.is_some() {
        "icon"
    } else {
        "spinner"
    });
    let label = gtk::Label::builder()
        .label(&item.label)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(0.0)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_top(0)
        .margin_bottom(0)
        .margin_start(0)
        .margin_end(0)
        .build();
    content.append(&icon_stack);
    if let Some(color) = item.color.as_deref().and_then(parse_hex_color) {
        content.append(&color_dot(color));
    }
    content.append(&label);

    let row = gtk::ListBoxRow::builder().child(&content).build();
    row.set_widget_name(&item.id);
    row
}

fn color_dot((red, green, blue, alpha): (f64, f64, f64, f64)) -> gtk::DrawingArea {
    let dot = gtk::DrawingArea::builder()
        .width_request(8)
        .height_request(8)
        .valign(gtk::Align::Center)
        .build();
    dot.set_draw_func(move |_, context, width, height| {
        let radius = (width.min(height) as f64 / 2.0 - 0.5).max(1.0);
        context.arc(
            width as f64 / 2.0,
            height as f64 / 2.0,
            radius,
            0.0,
            std::f64::consts::TAU,
        );
        context.set_source_rgba(red, green, blue, alpha);
        let _ = context.fill();
    });
    dot
}

fn parse_hex_color(color: &str) -> Option<(f64, f64, f64, f64)> {
    let hex = color.strip_prefix('#')?;
    let (red, green, blue, alpha) = match hex.len() {
        3 | 4 => {
            let mut chars = hex.chars();
            let red = doubled_hex(chars.next()?)?;
            let green = doubled_hex(chars.next()?)?;
            let blue = doubled_hex(chars.next()?)?;
            let alpha = if let Some(ch) = chars.next() {
                doubled_hex(ch)?
            } else {
                255
            };
            (red, green, blue, alpha)
        }
        6 | 8 => {
            let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let alpha = if hex.len() == 8 {
                u8::from_str_radix(&hex[6..8], 16).ok()?
            } else {
                255
            };
            (red, green, blue, alpha)
        }
        _ => return None,
    };

    Some((
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
        f64::from(alpha) / 255.0,
    ))
}

fn doubled_hex(ch: char) -> Option<u8> {
    let value = ch.to_digit(16)? as u8;
    Some(value * 16 + value)
}

fn find_stack(widget: &gtk::Widget) -> Option<gtk::Stack> {
    if let Ok(stack) = widget.clone().downcast::<gtk::Stack>() {
        return Some(stack);
    }

    let mut child = widget.first_child();
    while let Some(w) = child {
        if let Some(stack) = find_stack(&w) {
            return Some(stack);
        }
        child = w.next_sibling();
    }

    None
}

fn find_image(widget: &gtk::Widget) -> Option<gtk::Image> {
    if let Ok(img) = widget.clone().downcast::<gtk::Image>() {
        return Some(img);
    }

    let mut child = widget.first_child();
    while let Some(w) = child {
        if let Some(img) = find_image(&w) {
            return Some(img);
        }
        child = w.next_sibling();
    }

    None
}
