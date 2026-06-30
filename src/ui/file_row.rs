use super::{file_status, widgets};
use adw::prelude::*;

pub(super) fn changed_file_row(path: &str, status: &str, active: bool) -> gtk::ListBoxRow {
    let check_button = gtk::CheckButton::builder()
        .active(active)
        .valign(gtk::Align::Center)
        .build();
    let row = file_row(path, status, Some(&check_button));
    row
}

pub(super) fn update_changed_file_row_status(row: &gtk::ListBoxRow, status: &str) {
    let Some(content) = row.child().and_downcast::<gtk::Box>() else {
        return;
    };

    if let Some(icon) = content.last_child() {
        content.remove(&icon);
    }
    content.append(&file_status::icon(status));
}

pub(super) fn history_file_content(path: &str, status: &str) -> gtk::Box {
    file_row_content(path, status, None)
}

fn file_row(path: &str, status: &str, check_button: Option<&gtk::CheckButton>) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::builder()
        .child(&file_row_content(path, status, check_button))
        .build();
    row.set_widget_name(path);
    row
}

fn file_row_content(path: &str, status: &str, check_button: Option<&gtk::CheckButton>) -> gtk::Box {
    let title = widgets::heading(path);
    title.set_wrap(false);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_width_chars(1);
    title.set_hexpand(true);
    title.set_xalign(0.0);

    let status_icon = file_status::icon(status);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_top(2)
        .margin_bottom(2)
        .margin_start(2)
        .margin_end(6)
        .build();

    if let Some(check_button) = check_button {
        content.append(check_button);
    }
    content.append(&title);
    content.append(&status_icon);
    content
}
