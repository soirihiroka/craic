use super::CodexChatAction;
use super::view::ActionCallback;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn codex_tools_menu(callbacks: Rc<RefCell<Vec<ActionCallback>>>) -> gtk::MenuButton {
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
            "Thread goal",
            "emblem-default-symbolic",
            CodexChatAction::ShowThreadGoal,
        ),
        (
            "Run shell command",
            "utilities-terminal-symbolic",
            CodexChatAction::RunShellCommand,
        ),
        (
            "Background terminals",
            "view-grid-symbolic",
            CodexChatAction::ShowBackgroundTerminals,
        ),
        (
            "Skills",
            "applications-education-symbolic",
            CodexChatAction::ShowSkills,
        ),
        (
            "MCP servers",
            "network-server-symbolic",
            CodexChatAction::ShowMcpServers,
        ),
        (
            "Apps & connectors",
            "application-x-addon-symbolic",
            CodexChatAction::ShowApps,
        ),
        (
            "Plugins",
            "application-x-addon-symbolic",
            CodexChatAction::ShowPlugins,
        ),
        (
            "Experimental features",
            "applications-science-symbolic",
            CodexChatAction::ShowExperimentalFeatures,
        ),
        (
            "Account & usage",
            "avatar-default-symbolic",
            CodexChatAction::ShowAccountUsage,
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
                for callback in callbacks.borrow().iter() {
                    callback(action.clone());
                }
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
            }
        });
        content.append(&command);
    }
    popover.set_child(Some(&content));
    let button = gtk::MenuButton::builder()
        .icon_name("preferences-system-symbolic")
        .tooltip_text("Codex tools")
        .popover(&popover)
        .build();
    button.update_property(&[gtk::accessible::Property::Label("Codex tools")]);
    button.add_css_class("flat");
    button
}
