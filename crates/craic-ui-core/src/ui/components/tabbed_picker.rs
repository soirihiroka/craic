use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabbedPickerItem {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon_name: Option<String>,
    pub selected: bool,
}

impl TabbedPickerItem {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        icon_name: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            icon_name: Some(icon_name.into()),
            selected: false,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabbedPickerGroup {
    pub title: Option<String>,
    pub items: Vec<TabbedPickerItem>,
}

impl TabbedPickerGroup {
    pub fn new(title: impl Into<String>, items: Vec<TabbedPickerItem>) -> Self {
        Self {
            title: Some(title.into()),
            items,
        }
    }

    pub fn unlabelled(items: Vec<TabbedPickerItem>) -> Self {
        Self { title: None, items }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabbedPickerStatus {
    Ready,
    Loading(String),
    Empty(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabbedPickerTab {
    pub id: String,
    pub title: String,
    pub badge: Option<usize>,
    pub groups: Vec<TabbedPickerGroup>,
    pub status: TabbedPickerStatus,
}

impl TabbedPickerTab {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        groups: Vec<TabbedPickerGroup>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            badge: None,
            groups,
            status: TabbedPickerStatus::Ready,
        }
    }

    pub fn badge(mut self, badge: Option<usize>) -> Self {
        self.badge = badge;
        self
    }

    pub fn status(mut self, status: TabbedPickerStatus) -> Self {
        self.status = status;
        self
    }
}

#[derive(Clone)]
pub struct TabbedPicker {
    pub button: gtk::MenuButton,
    pub action_button: gtk::Button,
    footer_button: gtk::Button,
    footer_icon: gtk::Image,
    footer_label: gtk::Label,
    popover: gtk::Popover,
    stack: gtk::Stack,
    search_entry: gtk::SearchEntry,
    button_label: gtk::Label,
    tabs: Rc<RefCell<Vec<TabbedPickerTab>>>,
    activation_handlers: Rc<RefCell<Vec<Rc<dyn Fn(String)>>>>,
}

impl TabbedPicker {
    pub fn new(
        placeholder: &str,
        action_tooltip: &str,
        button_label: &str,
        button_icon_name: &str,
        button_tooltip: &str,
        tabs: Vec<TabbedPickerTab>,
    ) -> Self {
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(placeholder)
            .hexpand(true)
            .build();
        let action_button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(action_tooltip)
            .build();
        action_button.add_css_class("flat");

        let search_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        search_row.append(&search_entry);
        search_row.append(&action_button);

        let stack = gtk::Stack::builder().vhomogeneous(false).build();
        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));
        switcher.set_margin_top(6);
        switcher.set_margin_start(6);
        switcher.set_margin_end(6);

        let footer_icon = gtk::Image::builder().pixel_size(16).build();
        let footer_label = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .hexpand(true)
            .build();
        let footer_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk::Align::Center)
            .build();
        footer_content.append(&footer_icon);
        footer_content.append(&footer_label);
        let footer_button = gtk::Button::builder()
            .child(&footer_content)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .visible(false)
            .build();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();
        content.append(&switcher);
        content.append(&search_row);
        content.append(&stack);
        content.append(&footer_button);

        let popover = gtk::Popover::builder()
            .width_request(365)
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
        let button_label = gtk::Label::builder()
            .label(button_label)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(28)
            .build();
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
            action_button,
            footer_button,
            footer_icon,
            footer_label,
            popover,
            stack,
            search_entry,
            button_label,
            tabs: Rc::new(RefCell::new(tabs)),
            activation_handlers: Rc::new(RefCell::new(Vec::new())),
        };
        picker.refresh();
        picker.connect_search();
        picker
    }

    pub fn set_button_label(&self, label: &str) {
        self.button_label.set_label(label);
    }

    pub fn set_footer(&self, icon_name: &str, markup: &str, tooltip: &str) {
        self.footer_icon.set_icon_name(Some(icon_name));
        self.footer_label.set_markup(markup);
        self.footer_button.set_tooltip_text(Some(tooltip));
        self.footer_button.set_visible(true);
    }

    pub fn set_footer_visible(&self, visible: bool) {
        self.footer_button.set_visible(visible);
    }

    pub fn set_tab(&self, tab: TabbedPickerTab) {
        let mut tabs = self.tabs.borrow_mut();
        if let Some(existing) = tabs.iter_mut().find(|existing| existing.id == tab.id) {
            *existing = tab;
        } else {
            tabs.push(tab);
        }
        drop(tabs);
        self.refresh();
    }

    pub fn connect_item_activated<F: Fn(String) + 'static>(&self, callback: F) {
        self.activation_handlers
            .borrow_mut()
            .push(Rc::new(callback));
        self.refresh();
    }

    pub fn connect_action_clicked<F: Fn() + 'static>(&self, callback: F) {
        self.action_button.connect_clicked({
            let popover = self.button.popover();

            move |_| {
                if let Some(popover) = popover.as_ref() {
                    popover.popdown();
                }

                callback();
            }
        });
    }

    pub fn connect_footer_clicked<F: Fn() + 'static>(&self, callback: F) {
        self.footer_button.connect_clicked({
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

    fn connect_search(&self) {
        self.search_entry.connect_search_changed({
            let picker = self.clone();

            move |_| picker.refresh()
        });
    }

    fn refresh(&self) {
        let active = self
            .stack
            .visible_child_name()
            .map(|name| name.to_string())
            .or_else(|| self.tabs.borrow().first().map(|tab| tab.id.clone()));

        while let Some(child) = self.stack.first_child() {
            self.stack.remove(&child);
        }

        let filter = self.search_entry.text().trim().to_lowercase();
        for tab in self.tabs.borrow().iter() {
            let list = gtk::ListBox::new();
            list.set_selection_mode(gtk::SelectionMode::Single);
            list.add_css_class("navigation-sidebar");
            fill_tab(&list, tab, &filter);
            for callback in self.activation_handlers.borrow().iter() {
                connect_list_activated(&list, callback.clone(), self.button.popover());
            }

            let scroller = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .min_content_height(120)
                .max_content_height(300)
                .propagate_natural_height(true)
                .child(&list)
                .build();

            self.stack
                .add_titled(&scroller, Some(&tab.id), &tab_label(tab));
        }

        if let Some(active) = active {
            if self
                .tabs
                .borrow()
                .iter()
                .any(|tab| tab.id.as_str() == active.as_str())
            {
                self.stack.set_visible_child_name(&active);
            }
        }
    }
}

fn connect_list_activated(
    list: &gtk::ListBox,
    callback: Rc<dyn Fn(String)>,
    popover: Option<gtk::Popover>,
) {
    list.connect_row_activated(move |_, row| {
        let id = row.widget_name().to_string();
        if id.is_empty() {
            return;
        }

        if let Some(popover) = popover.as_ref() {
            popover.popdown();
        }

        callback(id);
    });
}

fn fill_tab(list: &gtk::ListBox, tab: &TabbedPickerTab, filter: &str) {
    match &tab.status {
        TabbedPickerStatus::Loading(message) => {
            list.append(&message_row("view-refresh-symbolic", message));
            return;
        }
        TabbedPickerStatus::Error(message) => {
            list.append(&message_row("dialog-warning-symbolic", message));
            return;
        }
        TabbedPickerStatus::Empty(message)
            if tab.groups.iter().all(|group| group.items.is_empty()) =>
        {
            list.append(&message_row("dialog-information-symbolic", message));
            return;
        }
        _ => {}
    }

    let mut visible = 0;
    for group in tab.groups.iter() {
        let items = group
            .items
            .iter()
            .filter(|item| item_matches(item, filter))
            .collect::<Vec<_>>();
        if items.is_empty() {
            continue;
        }

        if let Some(title) = group.title.as_ref() {
            list.append(&header_row(title));
        }

        for item in items {
            visible += 1;
            list.append(&item_row(item));
        }
    }

    if visible == 0 {
        list.append(&message_row("edit-find-symbolic", "No matching items."));
    }
}

fn item_matches(item: &TabbedPickerItem, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }

    item.title.to_lowercase().contains(filter)
        || item
            .subtitle
            .as_ref()
            .is_some_and(|subtitle| subtitle.to_lowercase().contains(filter))
}

fn tab_label(tab: &TabbedPickerTab) -> String {
    match tab.badge {
        Some(count) if count > 0 => format!("{} ({count})", tab.title),
        _ => tab.title.clone(),
    }
}

fn header_row(title: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .margin_top(8)
        .margin_bottom(3)
        .margin_start(6)
        .margin_end(6)
        .build();
    label.add_css_class("heading");
    let row = gtk::ListBoxRow::builder()
        .child(&label)
        .selectable(false)
        .build();
    row.set_activatable(false);
    row
}

fn item_row(item: &TabbedPickerItem) -> gtk::ListBoxRow {
    let icon_name = item
        .icon_name
        .as_deref()
        .unwrap_or("text-x-generic-symbolic");
    let icon = gtk::Image::builder()
        .icon_name(icon_name)
        .pixel_size(16)
        .valign(gtk::Align::Center)
        .build();
    let title = gtk::Label::builder()
        .label(&item.title)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(0.0)
        .build();
    if item.selected {
        title.add_css_class("heading");
    }

    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(1)
        .hexpand(true)
        .build();
    labels.append(&title);
    if let Some(subtitle) = item.subtitle.as_ref() {
        let subtitle = gtk::Label::builder()
            .label(subtitle)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .xalign(0.0)
            .build();
        subtitle.add_css_class("dim-label");
        labels.append(&subtitle);
    }

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(3)
        .margin_bottom(3)
        .margin_start(6)
        .margin_end(6)
        .build();
    content.append(&icon);
    content.append(&labels);

    let row = gtk::ListBoxRow::builder().child(&content).build();
    row.set_widget_name(&item.id);
    row
}

fn message_row(icon_name: &str, message: &str) -> gtk::ListBoxRow {
    let icon = gtk::Image::builder()
        .icon_name(icon_name)
        .pixel_size(16)
        .valign(gtk::Align::Center)
        .build();
    let label = gtk::Label::builder()
        .label(message)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .wrap(true)
        .xalign(0.0)
        .build();
    label.add_css_class("dim-label");
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(6)
        .margin_end(6)
        .build();
    content.append(&icon);
    content.append(&label);
    let row = gtk::ListBoxRow::builder()
        .child(&content)
        .selectable(false)
        .build();
    row.set_activatable(false);
    row
}
