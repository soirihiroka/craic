use super::provider;
use crate::config;
use crate::git::FileComparison;
use crate::markdown_lint::MarkdownLintIssue;
use crate::spellcheck::SpellcheckIssue;
use crate::system::FileNodePath;
use crate::ui::components::markdown_preview::MarkdownPreview as AdwMarkdownPreview;
use crate::ui::content::{binary_preview, code_editor, folder_view};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui::pages::file) struct PreviewLoadToken(u64);

pub(super) struct RightPane {
    pub(super) root: gtk::Box,
    title_label: gtk::Label,
    subtitle_label: gtk::Label,
    stack: gtk::Stack,
    preview_generation: Cell<u64>,
    provider_loading_label: gtk::Label,
    editor_loading: gtk::Box,
    pub(super) folder_view: folder_view::FolderView,
    pub(super) file_editor: code_editor::CodeEditor,
    pub(in crate::ui::pages::file) file_editor_path: Rc<RefCell<Option<FileNodePath>>>,
    pub(in crate::ui::pages::file) file_editor_disk_signature:
        Rc<RefCell<Option<provider::DiskSignature>>>,
    pub(in crate::ui::pages::file) file_editor_writable: Rc<Cell<bool>>,
    pub(in crate::ui::pages::file) file_view_split: gtk::Paned,
    pub(in crate::ui::pages::file) file_svg_preview: Rc<super::provider::svg::SvgPreview>,
    pub(in crate::ui::pages::file) file_markdown_preview: Rc<AdwMarkdownPreview>,
    pub(in crate::ui::pages::file) file_markdown_status: gtk::Box,
    pub(in crate::ui::pages::file) file_media_preview: Rc<super::provider::media::MediaPreview>,
    pub(in crate::ui::pages::file) file_notebook_preview:
        Rc<super::provider::notebook::NotebookPreview>,
    pub(in crate::ui::pages::file) file_font_preview: binary_preview::BinaryPreviewWidgets,
    pub(in crate::ui::pages::file) file_pdf_preview: binary_preview::BinaryPreviewWidgets,
    pub(in crate::ui::pages::file) file_sqlite_preview: Rc<super::provider::sqlite::SqlitePreview>,
    status_label: gtk::Label,
}

impl RightPane {
    pub(super) fn new() -> Self {
        let title_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading", "bold"])
            .build();
        let subtitle_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label"])
            .build();
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(14)
            .margin_end(14)
            .build();
        header.append(&title_label);
        header.append(&subtitle_label);

        let file_editor = code_editor::CodeEditor::new("", "");
        file_editor.set_font_size(config::load().font_sizes.editor);
        file_editor.set_editable(false);
        file_editor.root.set_vexpand(true);
        let editor_loading = loading_screen("Loading file preview...");

        let file_svg_preview = super::provider::svg::SvgPreview::new();
        let file_markdown_preview = AdwMarkdownPreview::new();
        let markdown_status_label = gtk::Label::builder()
            .label("No markdown preview.")
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .build();
        let file_markdown_status = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        file_markdown_status.append(&markdown_status_label);
        let file_media_preview = super::provider::media::MediaPreview::new();
        let file_notebook_preview = super::provider::notebook::NotebookPreview::new();
        let file_font_preview = binary_preview::BinaryPreviewWidgets::new("Font");
        let file_pdf_preview = binary_preview::BinaryPreviewWidgets::new("PDF");
        let file_sqlite_preview = super::provider::sqlite::SqlitePreview::new();
        install_markdown_scroll_sync(&file_editor, &file_markdown_preview);

        let file_view_split = gtk::Paned::new(gtk::Orientation::Horizontal);
        file_view_split.set_start_child(Some(&file_editor.root));
        file_view_split.set_resize_start_child(true);
        file_view_split.set_shrink_start_child(false);
        file_view_split.set_resize_end_child(true);
        file_view_split.set_shrink_end_child(true);
        file_view_split.set_position(640);

        let folder_view = folder_view::FolderView::new();

        let status_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .selectable(true)
            .build();
        let status_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        let status_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .min_content_height(120)
            .child(&status_label)
            .build();
        status_box.append(&status_scroller);

        let provider_loading_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .build();
        let provider_loading = loading_screen_with_label(&provider_loading_label);

        let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.add_named(&file_view_split, Some("editor"));
        stack.add_named(&folder_view.root, Some("folder"));
        stack.add_named(&file_media_preview.root, Some("media"));
        stack.add_named(&file_notebook_preview.root, Some("notebook"));
        stack.add_named(&file_font_preview.root, Some("font"));
        stack.add_named(&file_pdf_preview.root, Some("pdf"));
        stack.add_named(&file_sqlite_preview.root, Some("sqlite"));
        stack.add_named(&status_box, Some("status"));
        stack.add_named(&provider_loading, Some("provider-loading"));
        stack.set_visible_child_name("folder");

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&header);
        root.append(&stack);

        Self {
            root,
            title_label,
            subtitle_label,
            stack,
            preview_generation: Cell::new(0),
            provider_loading_label,
            editor_loading,
            folder_view,
            file_editor,
            file_editor_path: Rc::new(RefCell::new(None)),
            file_editor_disk_signature: Rc::new(RefCell::new(None)),
            file_editor_writable: Rc::new(Cell::new(false)),
            file_view_split,
            file_svg_preview,
            file_markdown_preview,
            file_markdown_status,
            file_media_preview,
            file_notebook_preview,
            file_font_preview,
            file_pdf_preview,
            file_sqlite_preview,
            status_label,
        }
    }

    pub(in crate::ui::pages::file) fn begin_preview_load(
        &self,
        file_path: &str,
    ) -> PreviewLoadToken {
        let generation = self.preview_generation.get().wrapping_add(1).max(1);
        self.preview_generation.set(generation);
        log::debug!("begin preview load file_path={file_path}");
        self.set_title(file_path, file_path);
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
        self.status_label.set_text("");
        PreviewLoadToken(generation)
    }

    pub(in crate::ui::pages::file) fn is_current_load(&self, token: PreviewLoadToken) -> bool {
        self.preview_generation.get() == token.0
    }

    pub(in crate::ui::pages::file) fn show_provider_loading(
        &self,
        file_path: &str,
        preview_kind: &str,
    ) {
        let message = format!("Loading {preview_kind} preview...");
        self.show_provider_loading_message(file_path, &message);
    }

    pub(in crate::ui::pages::file) fn show_provider_loading_message(
        &self,
        file_path: &str,
        message: &str,
    ) {
        log::debug!("show preview loading file_path={file_path} message={message}");
        self.set_title(file_path, file_path);
        self.provider_loading_label.set_text(message);
        self.stack.set_visible_child_name("provider-loading");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub(in crate::ui::pages::file) fn show_editor_loading(
        &self,
        file_path: &str,
        preview_kind: &str,
    ) {
        let message = format!("Loading {preview_kind} preview...");
        log::debug!("show editor preview loading file_path={file_path} message={message}");
        self.set_title(file_path, file_path);
        if let Some(label) = self
            .editor_loading
            .last_child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        {
            label.set_text(&message);
        }
        self.stack.set_visible_child_name("editor");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.editor_loading));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub(super) fn show_unavailable(&self, file_path: &str, message: &str) {
        self.cancel_preview_load();
        self.set_title(file_path, "");
        self.stack.set_visible_child_name("status");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
        self.status_label.set_text(message);
    }

    pub(super) fn show_folder_info(
        &self,
        workspace_root: &str,
        folder_path: &str,
        file_count: usize,
        folder_count: usize,
    ) {
        let subtitle = if folder_path.is_empty() {
            "Workspace root".to_string()
        } else {
            folder_path.to_string()
        };
        self.cancel_preview_load();
        self.set_title(folder_path, &subtitle);
        self.stack.set_visible_child_name("folder");
        self.clear_file_state();
        self.folder_view
            .set_info(workspace_root, folder_path, file_count, folder_count);
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub(in crate::ui::pages::file) fn show_editor(
        &self,
        node_path: &FileNodePath,
        file_path: &str,
        text: &str,
        disk_signature: provider::DiskSignature,
        writable: bool,
        comparison: Option<&FileComparison>,
        markdown_lint_issues: Vec<MarkdownLintIssue>,
        spellcheck_issues: Vec<SpellcheckIssue>,
    ) {
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("editor");
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_editor_path.replace(Some(node_path.clone()));
        self.file_editor_disk_signature
            .replace(Some(disk_signature));
        self.file_editor_writable.set(writable);

        let language = code_editor::language_hint_from_path(file_path);
        self.file_editor.set_document(&language, text);
        self.file_editor.set_editable(writable);
        self.file_editor.set_file_diff(comparison);
        self.file_editor
            .set_markdown_lint_issues(markdown_lint_issues);
        self.file_editor.set_spellcheck_issues(spellcheck_issues);
        self.clear_auxiliary_previews();
    }

    pub(in crate::ui::pages::file) fn show_media_preview(&self, file_path: &str, _subtitle: &str) {
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("media");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.file_sqlite_preview.clear();
    }

    pub(in crate::ui::pages::file) fn show_font_preview(&self, file_path: &str) {
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("font");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub(in crate::ui::pages::file) fn show_sqlite_preview(&self, file_path: &str) {
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("sqlite");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub(in crate::ui::pages::file) fn show_notebook_preview(&self, file_path: &str) {
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("notebook");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub(in crate::ui::pages::file) fn show_pdf_preview(&self, file_path: &str) {
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("pdf");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.file_media_preview.clear();
    }

    fn cancel_preview_load(&self) {
        let generation = self.preview_generation.get().wrapping_add(1).max(1);
        self.preview_generation.set(generation);
    }

    fn clear_file_state(&self) {
        self.file_editor_path.borrow_mut().take();
        self.file_editor_disk_signature.borrow_mut().take();
        self.file_editor_writable.set(false);
        self.file_editor.set_editable(false);
        self.file_editor.set_language("");
        self.file_editor.set_text("");
        self.file_editor.clear_file_diff();
        self.file_editor.set_markdown_lint_issues(Vec::new());
        self.file_editor.set_spellcheck_issues(Vec::new());
    }

    fn clear_auxiliary_previews(&self) {
        self.file_media_preview.clear();
        self.file_sqlite_preview.clear();
        self.file_notebook_preview.clear();
    }

    fn set_title(&self, file_path: &str, subtitle: &str) {
        let title = if file_path.is_empty() {
            "Workspace".to_string()
        } else {
            Path::new(file_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(file_path)
                .to_string()
        };

        self.title_label.set_text(&title);
        self.subtitle_label.set_text(subtitle);
    }
}

fn loading_screen(message: &str) -> gtk::Box {
    let label = gtk::Label::builder()
        .label(message)
        .halign(gtk::Align::Center)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["dim-label"])
        .build();

    loading_screen_with_label(&label)
}

pub(in crate::ui::pages::file) fn loading_screen_with_label(label: &gtk::Label) -> gtk::Box {
    let spinner = adw::Spinner::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    spinner.set_size_request(32, 32);

    let loading = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    loading.append(&spinner);
    loading.append(label);
    loading
}

fn install_markdown_scroll_sync(
    editor: &code_editor::CodeEditor,
    preview: &Rc<AdwMarkdownPreview>,
) {
    let updating_editor = Rc::new(Cell::new(false));

    let preview_adjustment = preview.root.vadjustment();

    editor.connect_scroll_changed({
        let editor = editor.clone();
        let preview = Rc::clone(preview);
        let updating_editor = Rc::clone(&updating_editor);
        move |_| {
            if updating_editor.get() {
                return;
            }

            let _ = preview.scroll_to_source_offset(editor.source_offset_at_scroll_top());
        }
    });

    preview_adjustment.connect_value_changed({
        let editor = editor.clone();
        let preview = Rc::clone(preview);
        let updating_editor = Rc::clone(&updating_editor);
        move |_| {
            updating_editor.set(true);
            if let Some(offset) = preview.source_offset_at_viewport_top() {
                editor.set_source_offset_at_scroll_top(offset);
            }
            updating_editor.set(false);
        }
    });
}
