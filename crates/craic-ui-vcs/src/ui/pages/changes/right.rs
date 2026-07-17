use super::super::preview_reconcile::{
    RightPreviewReconciler, RightPreviewState, binary_state, diff_state, should_update_preview,
    unavailable_state,
};
use crate::git::{BytesComparison, FileComparison, RepositorySnapshot};
use crate::ui::content::binary_preview::{
    BinaryPreviewWidgets, set_audio_preview, set_font_preview, set_image_preview, set_pdf_preview,
    set_unavailable_preview, set_video_preview,
};
use crate::ui::content::diff_view::DiffView;
use crate::ui::content::{self, SuggestionsActions, SuggestionsPanel};
use crate::ui::file_type::PreviewKind;
use adw::prelude::*;
use std::cell::RefCell;

pub struct ChangesRight {
    pub root: gtk::Stack,
    title: gtk::Label,
    subtitle: gtk::Label,
    pub initialize_button: gtk::Button,
    initialize_card: gtk::Box,
    diff: DiffView,
    file_preview: BinaryPreviewWidgets,
    pub suggestions_actions: SuggestionsActions,
    preview_reconciler: RefCell<RightPreviewReconciler>,
}

impl ChangesRight {
    pub fn new() -> Self {
        let title = crate::ui::widgets::title("No local changes");
        let subtitle = crate::ui::widgets::muted("Working tree is clean.");
        let suggestions = SuggestionsPanel::new();
        let initialize_button = gtk::Button::builder()
            .label("Initialize")
            .valign(gtk::Align::Center)
            .build();
        initialize_button.add_css_class("suggested-action");
        let initialize_card = initialize_repository_card(&initialize_button);
        initialize_card.set_visible(false);
        let suggestions_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .build();
        suggestions_box.append(&initialize_card);
        suggestions_box.append(&suggestions.root);
        let suggestions_content =
            content::centered_page(content::page(&title, &subtitle, &suggestions_box));

        let diff = DiffView::new("File");
        let file_preview = BinaryPreviewWidgets::new("File");
        let loading_spinner = adw::Spinner::builder()
            .width_request(24)
            .height_request(24)
            .halign(gtk::Align::Center)
            .build();
        let loading_title = crate::ui::widgets::title("Loading diff");
        let loading_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .build();
        loading_content.append(&loading_spinner);
        loading_content.append(&loading_title);

        let root = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        root.add_named(&suggestions_content, Some("suggestions"));
        root.add_named(&loading_content, Some("loading"));
        root.add_named(&diff.root, Some("diff"));
        root.add_named(&file_preview.root, Some("preview"));
        root.set_visible_child_name("suggestions");

        Self {
            root,
            title,
            subtitle,
            initialize_button,
            initialize_card,
            diff,
            file_preview,
            suggestions_actions: suggestions.actions,
            preview_reconciler: RefCell::new(RightPreviewReconciler::new()),
        }
    }

    pub fn update(&self, snapshot: &RepositorySnapshot, action_running: bool) {
        self.initialize_card.set_visible(false);
        let changed = snapshot.changed_files.len();
        self.title.set_label(&match changed {
            0 => "No local changes".to_string(),
            1 => "1 changed file".to_string(),
            count => format!("{count} changed files"),
        });
        self.subtitle
            .set_label(&format!("{} on {}", snapshot.name, snapshot.branch));
        self.configure_git_action(snapshot, action_running);
    }

    pub fn set_error(&self, message: &str) {
        self.initialize_card.set_visible(false);
        self.title.set_label("Repository unavailable");
        self.subtitle.set_label(message);
        self.show_home();
    }

    pub fn show_initialize_repository(&self) {
        self.title.set_label("Repository not initialized");
        self.subtitle
            .set_label("Initialize Git to track changes in this workspace.");
        self.initialize_card.set_visible(true);
        self.suggestions_actions.git_card.set_visible(false);
        self.show_home();
    }

    pub fn show_home(&self) {
        if !should_update_preview(&self.preview_reconciler, RightPreviewState::Home, "changes") {
            return;
        }
        self.root.set_visible_child_name("suggestions");
    }

    pub fn show_loading(&self, file_path: &str) {
        if !should_update_preview(
            &self.preview_reconciler,
            RightPreviewState::Loading {
                file_path: file_path.to_string(),
            },
            "changes",
        ) {
            return;
        }
        self.root.set_visible_child_name("loading");
    }

    pub fn show_comparison(&self, file_path: &str, comparison: &FileComparison) {
        if !should_update_preview(
            &self.preview_reconciler,
            diff_state(file_path, comparison),
            "changes",
        ) {
            return;
        }
        self.diff.set_diff(file_path, comparison);
        self.root.set_visible_child_name("diff");
    }

    pub fn toggle_search(&self) -> bool {
        if self.root.visible_child_name().as_deref() != Some("diff") {
            return false;
        }
        self.diff.toggle_search();
        true
    }

    pub fn show_binary_comparison(&self, file_path: &str, comparison: &BytesComparison) {
        if !should_update_preview(
            &self.preview_reconciler,
            binary_state(file_path, comparison),
            "changes",
        ) {
            return;
        }
        match crate::ui::file_type::preview_kind_for_path(file_path, false) {
            PreviewKind::Image => set_image_preview(&self.file_preview, file_path, comparison),
            PreviewKind::Audio => set_audio_preview(&self.file_preview, file_path, comparison),
            PreviewKind::Video => set_video_preview(&self.file_preview, file_path, comparison),
            PreviewKind::Font => set_font_preview(&self.file_preview, file_path, comparison),
            PreviewKind::Pdf => set_pdf_preview(&self.file_preview, file_path, comparison),
            _ => {
                set_unavailable_preview(&self.file_preview, file_path, "Preview unavailable.");
                self.root.set_visible_child_name("preview");
                return;
            }
        }
        self.root.set_visible_child_name("preview");
    }

    pub fn show_preview_unavailable(&self, file_path: &str, message: &str) {
        if !should_update_preview(
            &self.preview_reconciler,
            unavailable_state(file_path, message),
            "changes",
        ) {
            return;
        }
        set_unavailable_preview(&self.file_preview, file_path, message);
        self.root.set_visible_child_name("preview");
    }

    fn configure_git_action(&self, snapshot: &RepositorySnapshot, action_running: bool) {
        let Some(remote) = snapshot.remote_name.as_deref() else {
            self.suggestions_actions.git_card.set_visible(false);
            return;
        };

        if !snapshot.has_upstream {
            self.suggestions_actions
                .git_title
                .set_label("Publish your branch");
            self.suggestions_actions.git_subtitle.set_label(&format!(
                "Publish the local branch '{}' to the remote '{}' to share your commits.",
                snapshot.branch, remote
            ));
            self.suggestions_actions
                .git_button
                .set_label("Publish branch");
            self.suggestions_actions.git_card.set_visible(true);
            self.suggestions_actions
                .git_button
                .set_sensitive(!action_running);
        } else if snapshot.behind > 0 {
            let title = if snapshot.behind == 1 {
                "Pull 1 commit from remote".to_string()
            } else {
                format!("Pull {} commits from remote", snapshot.behind)
            };
            self.suggestions_actions.git_title.set_label(&title);
            self.suggestions_actions.git_subtitle.set_label(&format!(
                "The current branch '{}' has commits on the remote that do not exist locally.",
                snapshot.branch
            ));
            self.suggestions_actions
                .git_button
                .set_label(&format!("Pull {remote}"));
            self.suggestions_actions.git_card.set_visible(true);
            self.suggestions_actions
                .git_button
                .set_sensitive(!action_running);
        } else if snapshot.ahead > 0 {
            let title = if snapshot.ahead == 1 {
                "Push 1 commit to remote".to_string()
            } else {
                format!("Push {} commits to remote", snapshot.ahead)
            };
            self.suggestions_actions.git_title.set_label(&title);
            self.suggestions_actions
                .git_subtitle
                .set_label("You have local commits that haven't been pushed to the remote.");
            self.suggestions_actions
                .git_button
                .set_label(&format!("Push {remote}"));
            self.suggestions_actions.git_card.set_visible(true);
            self.suggestions_actions
                .git_button
                .set_sensitive(!action_running);
        } else {
            self.suggestions_actions.git_card.set_visible(false);
        }
    }
}

fn initialize_repository_card(button: &gtk::Button) -> gtk::Box {
    let title = gtk::Label::builder()
        .label("Initialize Git Repository")
        .xalign(0.0)
        .hexpand(true)
        .build();
    title.add_css_class("heading");

    let subtitle = gtk::Label::builder()
        .label("Create a Git repository under the current workspace.")
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();

    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    labels.append(&title);
    labels.append(&subtitle);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(20)
        .margin_end(20)
        .hexpand(true)
        .build();
    content.append(&labels);
    content.append(button);

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    card.add_css_class("card");
    card.append(&content);
    card
}
