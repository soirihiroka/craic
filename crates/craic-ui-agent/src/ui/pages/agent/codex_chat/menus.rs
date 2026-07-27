use super::CodexChatAction;
use super::view::ActionCallback;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn thread_command_menu(callbacks: Rc<RefCell<Vec<ActionCallback>>>) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .margin_top(3)
        .margin_bottom(3)
        .margin_start(4)
        .margin_end(4)
        .build();
    for (label, icon_name, action) in [
        (
            "Thread history",
            "document-open-recent-symbolic",
            CodexChatAction::ShowHistory,
        ),
        (
            "Fork thread",
            "vcs-branch-symbolic",
            CodexChatAction::ForkThread,
        ),
        (
            "Archive thread",
            "folder-symbolic",
            CodexChatAction::ArchiveThread,
        ),
        (
            "Compact context",
            "package-x-generic-symbolic",
            CodexChatAction::CompactThread,
        ),
        (
            "Start review",
            "emblem-ok-symbolic",
            CodexChatAction::StartReview,
        ),
        (
            "Roll back last turn",
            "edit-undo-symbolic",
            CodexChatAction::UndoLastTurn,
        ),
        (
            "Open changes / diff",
            "document-edit-symbolic",
            CodexChatAction::OpenChanges,
        ),
    ] {
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        let label = gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        row.append(&icon);
        row.append(&label);
        let command = gtk::Button::builder()
            .child(&row)
            .halign(gtk::Align::Fill)
            .build();
        command.add_css_class("flat");
        command.connect_clicked({
            let callbacks = callbacks.clone();
            let popover = popover.downgrade();
            move |_| {
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
                emit_action(&callbacks, action.clone());
            }
        });
        content.append(&command);
    }
    popover.set_child(Some(&content));
    let button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Thread actions")
        .popover(&popover)
        .build();
    button.add_css_class("flat");
    button
}

pub(super) fn add_context_menu(callbacks: Rc<RefCell<Vec<ActionCallback>>>) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .margin_top(3)
        .margin_bottom(3)
        .margin_start(4)
        .margin_end(4)
        .build();
    for (label, icon_name, action) in [
        (
            "Attach image or audio…",
            "applications-multimedia-symbolic",
            CodexChatAction::ChooseAttachment,
        ),
        (
            "Reference workspace file…",
            "document-open-symbolic",
            CodexChatAction::ChooseMention,
        ),
        (
            "Reference workspace folder…",
            "folder-open-symbolic",
            CodexChatAction::ChooseMentionFolder,
        ),
    ] {
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        let label = gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        row.append(&icon);
        row.append(&label);
        let command = gtk::Button::builder()
            .child(&row)
            .halign(gtk::Align::Fill)
            .build();
        command.add_css_class("flat");
        command.connect_clicked({
            let callbacks = callbacks.clone();
            let popover = popover.downgrade();
            move |_| {
                emit_action(&callbacks, action.clone());
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
            }
        });
        content.append(&command);
    }
    popover.set_child(Some(&content));
    let button = gtk::MenuButton::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add context")
        .popover(&popover)
        .build();
    button.update_property(&[gtk::accessible::Property::Label("Add context")]);
    button.add_css_class("flat");
    button
}

fn emit_action(callbacks: &RefCell<Vec<ActionCallback>>, action: CodexChatAction) {
    for callback in callbacks.borrow().iter() {
        callback(action.clone());
    }
}
