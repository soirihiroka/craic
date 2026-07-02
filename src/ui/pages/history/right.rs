use super::super::preview_reconcile::{
    RightPreviewReconciler, binary_state, diff_state, should_update_preview, unavailable_state,
};
use crate::git::{BytesComparison, ChangedFile, Commit, FileComparison};
use crate::ui::content::binary_preview::{
    BinaryPreviewWidgets, set_audio_preview, set_font_preview, set_image_preview, set_pdf_preview,
    set_unavailable_preview, set_video_preview,
};
use crate::ui::content::diff_view::DiffView;
use crate::ui::{file_row, file_type::PreviewKind, left_clamp::LeftClamp, widgets};
use adw::prelude::*;
use gtk::{gdk, pango};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) struct HistoryRight {
    root: gtk::Box,
    title: gtk::Label,
    comment: gtk::Label,
    subtitle: gtk::Label,
    avatar: adw::Avatar,
    hash_group: gtk::Box,
    hash: gtk::Label,
    hash_copy_button: gtk::Button,
    time_group: gtk::Box,
    time: gtk::Label,
    stats_group: gtk::Box,
    added: gtk::Label,
    deleted: gtk::Label,
    hash_to_copy: Rc<RefCell<Option<String>>>,
    file_count: gtk::Label,
    files: gtk::ListBox,
    diff: DiffView,
    preview_stack: gtk::Stack,
    file_preview: BinaryPreviewWidgets,
    avatar_source: Rc<RefCell<Option<String>>>,
    preview_reconciler: RefCell<RightPreviewReconciler>,
}

impl HistoryRight {
    pub(super) fn new() -> Self {
        let title = widgets::title("Select a commit");
        title.set_wrap(true);
        title.set_wrap_mode(pango::WrapMode::WordChar);
        title.set_halign(gtk::Align::Fill);
        title.set_xalign(0.0);
        title.set_width_chars(1);
        title.set_ellipsize(pango::EllipsizeMode::None);
        title.set_selectable(true);
        title.set_hexpand(true);

        let comment = widgets::muted("");
        comment.set_wrap(true);
        comment.set_wrap_mode(pango::WrapMode::WordChar);
        comment.set_hexpand(true);
        comment.set_halign(gtk::Align::Fill);
        comment.set_width_chars(1);
        comment.set_xalign(0.0);
        comment.set_ellipsize(pango::EllipsizeMode::None);
        comment.set_selectable(true);
        comment.set_visible(false);

        let title_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        title_box.append(&title);
        title_box.append(&comment);

        let message_clamp = LeftClamp::new(&title_box);
        message_clamp.set_hexpand(true);
        message_clamp.set_halign(gtk::Align::Fill);

        let subtitle = widgets::muted("Choose a commit from the history list.");
        subtitle.set_wrap(false);
        subtitle.set_lines(1);
        subtitle.set_ellipsize(pango::EllipsizeMode::End);
        subtitle.set_width_chars(1);
        subtitle.set_max_width_chars(32);

        let file_count = widgets::muted("No commit selected");

        let avatar = adw::Avatar::builder()
            .size(32)
            .show_initials(true)
            .visible(false)
            .build();

        let hash = metadata_label("");
        hash.add_css_class("numeric");
        let hash_copy_button = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy full commit hash")
            .valign(gtk::Align::Center)
            .sensitive(false)
            .build();
        hash_copy_button.add_css_class("flat");

        let hash_group = metadata_group(false);
        hash_group.append(&separator_label());
        hash_group.append(&hash);
        hash_group.append(&hash_copy_button);

        let time = metadata_label("");
        let time_group = metadata_group(false);
        time_group.append(&separator_label());
        time_group.append(&time);

        let added = metadata_label("");
        added.add_css_class("numeric");
        added.add_css_class("success");
        let deleted = metadata_label("");
        deleted.add_css_class("numeric");
        deleted.add_css_class("error");
        let stats_group = metadata_group(false);
        stats_group.append(&separator_label());
        stats_group.append(&added);
        stats_group.append(&deleted);

        let hash_to_copy = Rc::new(RefCell::new(None::<String>));
        hash_copy_button.connect_clicked({
            let hash_to_copy = hash_to_copy.clone();

            move |_| {
                let Some(hash) = hash_to_copy.borrow().clone() else {
                    return;
                };
                let Some(display) = gdk::Display::default() else {
                    return;
                };
                display.clipboard().set_text(&hash);
            }
        });

        let metadata = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();
        metadata.append(&avatar);
        metadata.append(&subtitle);
        metadata.append(&hash_group);
        metadata.append(&time_group);
        metadata.append(&stats_group);

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(18)
            .margin_bottom(12)
            .margin_start(18)
            .margin_end(18)
            .build();
        header.append(&message_clamp);
        header.append(&metadata);

        let files = gtk::ListBox::new();
        files.set_selection_mode(gtk::SelectionMode::Single);
        files.add_css_class("navigation-sidebar");

        let files_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&files)
            .build();

        let files_header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(8)
            .margin_start(10)
            .margin_end(10)
            .margin_bottom(6)
            .build();
        files_header.append(&file_count);

        let files_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(280)
            .vexpand(true)
            .build();
        files_panel.append(&files_header);
        files_panel.append(&files_scroller);

        let diff = DiffView::new("Select a file");
        let file_preview = BinaryPreviewWidgets::new("Select a file");
        let preview_stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        preview_stack.add_named(&diff.root, Some("diff"));
        preview_stack.add_named(&file_preview.root, Some("preview"));
        preview_stack.set_visible_child_name("diff");

        let separator = gtk::Separator::new(gtk::Orientation::Vertical);
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .build();
        body.append(&files_panel);
        body.append(&separator);
        body.append(&preview_stack);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&header);
        root.append(&body);

        Self {
            root,
            title,
            comment,
            subtitle,
            avatar,
            hash_group,
            hash,
            hash_copy_button,
            time_group,
            time,
            stats_group,
            added,
            deleted,
            hash_to_copy,
            file_count,
            files,
            diff,
            preview_stack,
            file_preview,
            avatar_source: Rc::new(RefCell::new(None)),
            preview_reconciler: RefCell::new(RightPreviewReconciler::new()),
        }
    }

    pub(super) fn root(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub(super) fn show_empty(&self) {
        self.avatar.set_visible(false);
        self.avatar.set_widget_name("");
        self.avatar
            .set_custom_image(Option::<&gdk::Paintable>::None);
        self.avatar_source.borrow_mut().take();
        self.title.set_label("Select a commit");
        self.comment.set_text("");
        self.comment.set_visible(false);
        self.subtitle
            .set_label("Choose a commit from the history list.");
        self.hash_group.set_visible(false);
        self.time_group.set_visible(false);
        self.stats_group.set_visible(false);
        self.hash_to_copy.borrow_mut().take();
        self.hash_copy_button.set_sensitive(false);
        self.file_count.set_label("No commit selected");
        self.clear_files();
        self.preview_stack.set_visible_child_name("diff");
        self.diff.clear("Select a file");
    }

    pub(super) fn show_error(&self, message: &str) {
        self.show_empty();
        self.title.set_label("Repository unavailable");
        self.subtitle.set_label(message);
    }

    pub(super) fn connect_file_selected<F>(&self, callback: F)
    where
        F: Fn(Option<String>) + 'static,
    {
        self.files.connect_row_selected(move |_, row| {
            callback(row.and_then(row_path));
        });
    }

    pub(super) fn selected_file_path(&self) -> Option<String> {
        self.files.selected_row().and_then(|row| row_path(&row))
    }

    pub(super) fn show_commit(&self, commit: &Commit, files: &[ChangedFile]) {
        self.update_avatar(commit);
        self.title.set_label(&commit.subject);
        self.comment.set_visible(!commit.comment.is_empty());
        self.comment.set_text(&commit.comment);
        self.subtitle.set_label(&commit.author);
        self.hash.set_label(&commit.short_hash);
        self.time.set_label(&commit.relative_time);
        self.added.set_label(&format!("+{}", commit.insertions));
        self.deleted.set_label(&format!("-{}", commit.deletions));
        *self.hash_to_copy.borrow_mut() = Some(commit.hash.clone());
        self.hash_copy_button.set_sensitive(true);
        self.hash_group.set_visible(true);
        self.time_group.set_visible(true);
        self.stats_group.set_visible(true);
        self.file_count.set_label(&match files.len() {
            0 => "No changed files".to_string(),
            1 => "1 changed file".to_string(),
            count => format!("{count} changed files"),
        });
        self.fill_files(files);

        if !self.select_first_file() {
            self.preview_stack.set_visible_child_name("diff");
            self.diff.clear("No changed files");
        }
    }

    pub(super) fn show_comparison(&self, file_path: &str, comparison: &FileComparison) {
        if !should_update_preview(
            &self.preview_reconciler,
            diff_state(file_path, comparison),
            "history",
        ) {
            return;
        }
        self.preview_stack.set_visible_child_name("diff");
        self.diff.set_diff(file_path, comparison);
    }

    pub(super) fn toggle_search(&self) -> bool {
        if self.preview_stack.visible_child_name().as_deref() != Some("diff") {
            return false;
        }
        self.diff.toggle_search();
        true
    }

    pub(super) fn show_binary_comparison(&self, file_path: &str, comparison: &BytesComparison) {
        if !should_update_preview(
            &self.preview_reconciler,
            binary_state(file_path, comparison),
            "history",
        ) {
            return;
        }
        match crate::ui::file_type::preview_kind_for_path(file_path, false) {
            PreviewKind::Image => {
                set_image_preview(&self.file_preview, file_path, comparison);
            }
            PreviewKind::Audio => {
                set_audio_preview(&self.file_preview, file_path, comparison);
            }
            PreviewKind::Video => {
                set_video_preview(&self.file_preview, file_path, comparison);
            }
            PreviewKind::Font => {
                set_font_preview(&self.file_preview, file_path, comparison);
            }
            PreviewKind::Pdf => {
                set_pdf_preview(&self.file_preview, file_path, comparison);
            }
            _ => {
                set_unavailable_preview(&self.file_preview, file_path, "Preview unavailable.");
            }
        }
        self.preview_stack.set_visible_child_name("preview");
    }

    pub(super) fn show_preview_unavailable(&self, file_path: &str, message: &str) {
        if !should_update_preview(
            &self.preview_reconciler,
            unavailable_state(file_path, message),
            "history",
        ) {
            return;
        }
        set_unavailable_preview(&self.file_preview, file_path, message);
        self.preview_stack.set_visible_child_name("preview");
    }

    fn clear_files(&self) {
        self.files.unselect_all();
        while let Some(row) = self.files.row_at_index(0) {
            self.files.remove(&row);
        }
    }

    fn fill_files(&self, files: &[ChangedFile]) {
        self.clear_files();

        for file in files {
            let row = gtk::ListBoxRow::builder()
                .child(&file_row::history_file_content(&file.path, &file.status))
                .build();
            row.set_widget_name(&file.path);
            self.files.append(&row);
        }
    }

    fn select_first_file(&self) -> bool {
        let Some(row) = self.files.row_at_index(0) else {
            return false;
        };

        self.files.select_row(Some(&row));
        true
    }

    fn update_avatar(&self, commit: &Commit) {
        self.avatar.set_visible(true);
        self.avatar.set_text(Some(&commit.author));
        self.avatar.set_tooltip_text(Some(&commit.author));

        let Some(email) = commit
            .author_email
            .as_deref()
            .filter(|email| !email.is_empty())
        else {
            self.avatar.set_widget_name("");
            self.avatar
                .set_custom_image(Option::<&gdk::Paintable>::None);
            self.avatar_source.borrow_mut().take();
            return;
        };

        let source = widgets::AvatarSource::Email(email.to_string());
        let source_key = Some(source.key());
        if *self.avatar_source.borrow() == source_key {
            return;
        }

        *self.avatar_source.borrow_mut() = source_key;
        self.avatar.set_widget_name("");
        self.avatar
            .set_custom_image(Option::<&gdk::Paintable>::None);
        widgets::fetch_avatar(&self.avatar, source);
    }
}

fn row_path(row: &gtk::ListBoxRow) -> Option<String> {
    let path = row.widget_name().to_string();
    (!path.is_empty()).then_some(path)
}

fn metadata_label(text: &str) -> gtk::Label {
    let label = widgets::muted(text);
    label.set_wrap(false);
    label.set_lines(1);
    label.set_ellipsize(pango::EllipsizeMode::End);
    label.set_valign(gtk::Align::Center);
    label
}

fn metadata_group(visible: bool) -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .valign(gtk::Align::Center)
        .visible(visible)
        .build()
}

fn separator_label() -> gtk::Label {
    metadata_label("·")
}
