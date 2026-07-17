use crate::ui::pages::{PageBadge, PageRef};
use adw::prelude::*;

pub(in crate::ui) struct ModeSwitcher {
    pub(in crate::ui) root: gtk::Box,
    pub(in crate::ui) buttons: Vec<gtk::ToggleButton>,
    icons: Vec<gtk::Image>,
    spinners: Vec<adw::Spinner>,
    badges: Vec<gtk::Label>,
}

impl ModeSwitcher {
    pub(super) fn new(pages: &[PageRef]) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        root.add_css_class("linked");

        let mut buttons = Vec::new();
        let mut icons = Vec::new();
        let mut spinners = Vec::new();
        let mut badges = Vec::new();
        let mut group: Option<gtk::ToggleButton> = None;

        for (index, page) in pages.iter().enumerate() {
            let icon = gtk::Image::from_icon_name(page.icon_name());

            let content = gtk::Overlay::builder()
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .width_request(32)
                .height_request(26)
                .build();
            content.set_direction(gtk::TextDirection::Ltr);
            content.set_child(Some(&icon));

            let spinner = adw::Spinner::builder()
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .visible(false)
                .width_request(16)
                .height_request(16)
                .build();
            content.add_overlay(&spinner);
            content.set_measure_overlay(&spinner, false);

            let badge = gtk::Label::builder()
                .valign(gtk::Align::Start)
                .halign(gtk::Align::End)
                .width_request(14)
                .height_request(14)
                .xalign(0.5)
                .yalign(0.5)
                .visible(false)
                .build();
            badge.add_css_class(if page.label() == "Agents" {
                "agent-badge"
            } else {
                "changes-badge"
            });
            badge.add_css_class("numeric");
            content.add_overlay(&badge);
            content.set_measure_overlay(&badge, false);
            content.set_clip_overlay(&badge, false);

            let mut builder = gtk::ToggleButton::builder()
                .child(&content)
                .hexpand(true)
                .tooltip_text(page.label());
            if let Some(group) = group.as_ref() {
                builder = builder.group(group);
            }
            let button = builder.active(index == 0).build();
            if group.is_none() {
                group = Some(button.clone());
            }
            root.append(&button);
            buttons.push(button);
            icons.push(icon);
            spinners.push(spinner);
            badges.push(badge);
        }

        Self {
            root,
            buttons,
            icons,
            spinners,
            badges,
        }
    }

    pub(in crate::ui) fn update_badges(&self, pages: &[PageRef]) {
        for (index, page) in pages.iter().enumerate() {
            self.set_badge(index, page.badge());
        }
    }

    pub(super) fn set_badge(&self, index: usize, badge: Option<PageBadge>) {
        let Some(label) = self.badges.get(index) else {
            return;
        };
        if let Some(badge) = badge {
            label.set_label(badge.text());
            label.set_visible(true);
        } else {
            label.set_visible(false);
        }
    }

    pub(in crate::ui) fn clear_badges(&self) {
        for badge in &self.badges {
            badge.set_visible(false);
        }
    }

    pub(in crate::ui) fn set_refreshing(&self, index: usize, refreshing: bool) {
        let Some(icon) = self.icons.get(index) else {
            return;
        };
        let Some(spinner) = self.spinners.get(index) else {
            return;
        };

        icon.set_opacity(if refreshing { 0.35 } else { 1.0 });
        spinner.set_visible(refreshing);
    }
}
