use super::super::widgets;
use crate::{git::RepositorySnapshot, github};
use adw::prelude::*;
use gtk::gdk;
use std::cell::RefCell;

pub struct CommitPanel {
    pub root: gtk::Box,
    pub avatar: adw::Avatar,
    pub avatar_button: gtk::Button,
    pub remote_owner_warning: gtk::Image,
    pub summary_entry: gtk::Entry,
    pub description_view: gtk::TextView,
    pub generate_button: gtk::Button,
    pub generate_icon_stack: gtk::Stack,
    pub commit_button: gtk::Button,
    avatar_source: RefCell<Option<String>>,
}

impl CommitPanel {
    pub fn new() -> Self {
        let avatar = adw::Avatar::builder()
            .size(32)
            .text("Workspace")
            .show_initials(true)
            .tooltip_text("Git author")
            .build();
        let avatar_button = gtk::Button::builder()
            .child(&avatar)
            .tooltip_text("Select commit email")
            .width_request(40)
            .height_request(40)
            .valign(gtk::Align::Center)
            .build();
        avatar_button.add_css_class("flat");
        avatar_button.add_css_class("circular");

        let remote_owner_warning = gtk::Image::from_icon_name("dialog-warning-symbolic");
        remote_owner_warning.set_pixel_size(16);
        remote_owner_warning.add_css_class("warning");
        remote_owner_warning.set_visible(false);
        remote_owner_warning.set_tooltip_text(Some(
            "Git author owner mismatch warning can be disabled in workspace preferences.",
        ));

        let avatar_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        avatar_content.append(&avatar_button);
        avatar_content.append(&remote_owner_warning);

        let summary_entry = gtk::Entry::builder()
            .placeholder_text("Summary (required)")
            .hexpand(true)
            .build();
        let generate_content_size = 20;
        let generate_icon_size = 16;
        let generate_icon = gtk::Image::from_icon_name("dialog-information-symbolic");
        generate_icon.set_pixel_size(generate_icon_size);
        let generate_cancel_icon = gtk::Image::from_icon_name("process-stop-symbolic");
        generate_cancel_icon.set_pixel_size(generate_icon_size);
        let generate_spinner = adw::Spinner::new();
        generate_spinner.set_size_request(generate_content_size, generate_content_size);
        generate_spinner.set_halign(gtk::Align::Center);
        generate_spinner.set_valign(gtk::Align::Center);
        let generate_icon_stack = gtk::Stack::builder()
            .width_request(generate_content_size)
            .height_request(generate_content_size)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        generate_icon_stack.add_named(&generate_icon, Some("icon"));
        generate_icon_stack.add_named(&generate_cancel_icon, Some("cancel"));
        generate_icon_stack.add_named(&generate_spinner, Some("spinner"));
        generate_icon_stack.set_visible_child_name("icon");

        let generate_button = gtk::Button::builder()
            .child(&generate_icon_stack)
            .tooltip_text("Generate commit message")
            .width_request(32)
            .height_request(32)
            .valign(gtk::Align::Center)
            .sensitive(false)
            .build();
        generate_button.add_css_class("flat");
        generate_button.add_css_class("circular");
        let summary_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        summary_row.append(&avatar_content);
        summary_row.append(&summary_entry);
        summary_row.append(&generate_button);

        let description_view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        let description_frame = gtk::ScrolledWindow::builder()
            .height_request(120)
            .min_content_height(120)
            .vexpand(true)
            .child(&description_view)
            .build();

        let commit_button = gtk::Button::with_label("Commit to branch");
        commit_button.add_css_class("suggested-action");
        commit_button.set_sensitive(false);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(10)
            .margin_start(10)
            .margin_end(10)
            .build();
        root.append(&summary_row);
        root.append(&description_frame);
        root.append(&commit_button);

        Self {
            root,
            avatar,
            avatar_button,
            remote_owner_warning,
            summary_entry,
            description_view,
            generate_button,
            generate_icon_stack,
            commit_button,
            avatar_source: RefCell::new(None),
        }
    }

    pub fn set_branch(&self, branch: &str) {
        self.commit_button.set_label(&format!("Commit to {branch}"));
    }

    pub fn clear(&self) {
        self.commit_button.set_sensitive(false);
        self.avatar_source.borrow_mut().take();
        self.avatar.set_widget_name("");
        self.avatar
            .set_custom_image(Option::<&gdk::Paintable>::None);
        self.remote_owner_warning.set_visible(false);
        self.remote_owner_warning.set_tooltip_text(None);
    }

    pub fn update_avatar(&self, snapshot: &RepositorySnapshot) {
        let text = snapshot
            .user_name
            .as_deref()
            .unwrap_or(snapshot.name.as_str());
        self.avatar.set_text(Some(text));
        self.avatar
            .set_tooltip_text(Some(&format!("{text}\nClick to select commit email")));
        self.avatar_button
            .set_tooltip_text(Some(&format!("{text}\nClick to select commit email")));

        let warning = remote_author_warning_text(snapshot);
        self.remote_owner_warning.set_visible(warning.is_some());
        self.remote_owner_warning
            .set_tooltip_text(warning.as_deref());

        let source = snapshot
            .github_avatar_url
            .as_ref()
            .map(|url| widgets::AvatarSource::Url(url.clone()))
            .or_else(|| {
                snapshot
                    .user_email
                    .as_ref()
                    .map(|email| widgets::AvatarSource::Email(email.clone()))
            });
        let source_key = source.as_ref().map(widgets::AvatarSource::key);

        if *self.avatar_source.borrow() == source_key {
            return;
        }

        *self.avatar_source.borrow_mut() = source_key;
        self.avatar.set_widget_name("");
        self.avatar
            .set_custom_image(Option::<&gdk::Paintable>::None);

        if let Some(source) = source {
            widgets::fetch_avatar(&self.avatar, source);
        }
    }
}

fn remote_author_warning_text(snapshot: &RepositorySnapshot) -> Option<String> {
    if !snapshot.warn_if_remote_owner_mismatch {
        return None;
    }

    let remote_owner = snapshot.remote_owner.as_deref()?;
    let local_author = local_commit_identity(snapshot)?;

    if local_author.eq_ignore_ascii_case(remote_owner) {
        return None;
    }

    if let Some(local_email_login) = snapshot
        .user_email
        .as_deref()
        .and_then(|email| github::login_from_noreply_email(email))
    {
        if local_email_login.eq_ignore_ascii_case(remote_owner) {
            return None;
        }
    }

    log::debug!("remote owner mismatch warning: local={local_author} remote_owner={remote_owner}");

    Some(format!(
        "Current git author {local_author} does not match remote owner {remote_owner}."
    ))
}

fn local_commit_identity(snapshot: &RepositorySnapshot) -> Option<String> {
    snapshot
        .user_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            snapshot
                .user_email
                .as_deref()
                .and_then(github::login_from_noreply_email)
        })
}
