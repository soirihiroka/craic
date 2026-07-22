use super::provider;
use crate::config;
use crate::git::FileComparison;
use crate::markdown_lint::MarkdownLintIssue;
use crate::spellcheck::SpellcheckIssue;
use crate::system::FileNodePath;
use crate::ui::content::{binary_preview, code_editor, folder_view};
use adw::prelude::*;
use craic_dynamic_data::{TextFormat, parse_text};
use craic_ui_object_viewer::ObjectViewer;
use craic_ui_preview::markdown_preview::MarkdownPreview as AdwMarkdownPreview;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

const PREVIEW_LOADING_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewLoadToken(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreviewLoadingKind {
    Provider,
    Editor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileViewKind {
    Csv,
    DynamicData(TextFormat),
}

#[derive(Clone, Debug)]
struct PendingPreviewLoading {
    token: PreviewLoadToken,
    file_path: String,
    message: String,
    kind: PreviewLoadingKind,
    shown: bool,
}

pub struct RightPane {
    pub root: gtk::Box,
    title_label: gtk::Label,
    subtitle_label: gtk::Label,
    file_view_mode_switcher: gtk::Box,
    file_code_button: gtk::ToggleButton,
    file_view_button: gtk::ToggleButton,
    file_view_kind: Rc<Cell<Option<FileViewKind>>>,
    stack: gtk::Stack,
    preview_generation: Cell<u64>,
    pending_preview_loading: RefCell<Option<PendingPreviewLoading>>,
    provider_loading_label: gtk::Label,
    editor_loading: gtk::Box,
    pub folder_view: folder_view::FolderView,
    pub file_editor: code_editor::CodeEditor,
    pub file_editor_path: Rc<RefCell<Option<FileNodePath>>>,
    pub file_editor_disk_signature: Rc<RefCell<Option<provider::DiskSignature>>>,
    pub file_editor_writable: Rc<Cell<bool>>,
    pub file_view_split: gtk::Paned,
    pub file_svg_preview: Rc<super::provider::svg::SvgPreview>,
    pub file_html_preview: Rc<super::provider::html::HtmlPreview>,
    pub file_markdown_preview: Rc<AdwMarkdownPreview>,
    pub file_markdown_status: gtk::Box,
    pub file_media_preview: Rc<super::provider::media::MediaPreview>,
    pub file_notebook_preview: Rc<super::provider::notebook::NotebookPreview>,
    pub file_font_preview: binary_preview::BinaryPreviewWidgets,
    pub file_pdf_preview: binary_preview::BinaryPreviewWidgets,
    pub file_sqlite_preview: Rc<super::provider::sqlite::SqlitePreview>,
    file_csv_preview: Rc<super::provider::csv::CsvPreview>,
    file_object_preview: Rc<ObjectViewer>,
    pub file_safetensors_metadata_preview: gtk::TextView,
    status_label: gtk::Label,
}

impl RightPane {
    pub fn new() -> Self {
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
        let title_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .build();
        title_box.append(&title_label);
        title_box.append(&subtitle_label);

        let file_code_button = gtk::ToggleButton::builder()
            .label("Code")
            .active(true)
            .build();
        let file_view_button = gtk::ToggleButton::builder()
            .label("Table")
            .group(&file_code_button)
            .build();
        let file_view_kind = Rc::new(Cell::new(None));
        let file_view_mode_switcher = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        file_view_mode_switcher.add_css_class("linked");
        file_view_mode_switcher.append(&file_code_button);
        file_view_mode_switcher.append(&file_view_button);

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(14)
            .margin_end(14)
            .build();
        header.append(&title_box);
        header.append(&file_view_mode_switcher);

        let file_editor = code_editor::CodeEditor::new("", "");
        file_editor.set_font_size(config::load().font_sizes.editor);
        file_editor.set_editable(false);
        file_editor.root.set_vexpand(true);
        let editor_loading = loading_screen("Loading file preview...");

        let file_svg_preview = super::provider::svg::SvgPreview::new();
        let file_html_preview = super::provider::html::HtmlPreview::new();
        let file_markdown_preview = AdwMarkdownPreview::new();
        let markdown_status_label = gtk::Label::builder()
            .label("No rendered preview.")
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
        let file_csv_preview = super::provider::csv::CsvPreview::new();
        let file_object_preview = ObjectViewer::new();
        install_markdown_scroll_sync(&file_editor, &file_markdown_preview);
        let file_safetensors_metadata_preview = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .top_margin(12)
            .bottom_margin(12)
            .left_margin(12)
            .right_margin(12)
            .hexpand(true)
            .vexpand(true)
            .build();
        file_safetensors_metadata_preview.set_monospace(true);
        let file_safetensors_metadata_scroller = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .min_content_width(360)
            .child(&file_safetensors_metadata_preview)
            .build();
        file_safetensors_metadata_scroller.set_has_frame(false);

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
        stack.add_named(&file_safetensors_metadata_scroller, Some("safetensors"));
        stack.add_named(&folder_view.root, Some("folder"));
        stack.add_named(&file_media_preview.root, Some("media"));
        stack.add_named(&file_notebook_preview.root, Some("notebook"));
        stack.add_named(&file_font_preview.root, Some("font"));
        stack.add_named(&file_pdf_preview.root, Some("pdf"));
        stack.add_named(&file_sqlite_preview.root, Some("sqlite"));
        stack.add_named(&file_csv_preview.root, Some("csv-table"));
        stack.add_named(&file_object_preview.root, Some("object-view"));
        stack.add_named(&status_box, Some("status"));
        stack.add_named(&provider_loading, Some("provider-loading"));
        stack.set_visible_child_name("folder");

        file_code_button.connect_toggled({
            let stack = stack.clone();
            let switcher = file_view_mode_switcher.clone();
            let view_kind = Rc::clone(&file_view_kind);

            move |button| {
                if button.is_active() && switcher.is_visible() {
                    log::debug!(
                        "file preview display mode changed kind={:?} mode=code",
                        view_kind.get()
                    );
                    stack.set_visible_child_name("editor");
                }
            }
        });
        file_view_button.connect_toggled({
            let editor = file_editor.clone();
            let csv_preview = Rc::clone(&file_csv_preview);
            let object_preview = Rc::clone(&file_object_preview);
            let stack = stack.clone();
            let switcher = file_view_mode_switcher.clone();
            let view_kind = Rc::clone(&file_view_kind);

            move |button| {
                if !button.is_active() || !switcher.is_visible() {
                    return;
                }
                let Some(kind) = view_kind.get() else {
                    return;
                };
                let source = editor.document_text();
                log::debug!("file preview display mode changed kind={kind:?} mode=view");
                match kind {
                    FileViewKind::Csv => {
                        csv_preview.set_source(&source);
                        stack.set_visible_child_name("csv-table");
                    }
                    FileViewKind::DynamicData(format) => {
                        match parse_text(format, &source) {
                            Ok(document) => object_preview.set_document(document),
                            Err(error) => {
                                log::warn!("structured data preview parse failed: {error}");
                                object_preview.show_error(&error);
                            }
                        }
                        stack.set_visible_child_name("object-view");
                    }
                }
            }
        });

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
            file_view_mode_switcher,
            file_code_button,
            file_view_button,
            file_view_kind,
            stack,
            preview_generation: Cell::new(0),
            pending_preview_loading: RefCell::new(None),
            provider_loading_label,
            editor_loading,
            folder_view,
            file_editor,
            file_editor_path: Rc::new(RefCell::new(None)),
            file_editor_disk_signature: Rc::new(RefCell::new(None)),
            file_editor_writable: Rc::new(Cell::new(false)),
            file_view_split,
            file_svg_preview,
            file_html_preview,
            file_markdown_preview,
            file_markdown_status,
            file_media_preview,
            file_notebook_preview,
            file_font_preview,
            file_pdf_preview,
            file_sqlite_preview,
            file_csv_preview,
            file_object_preview,
            file_safetensors_metadata_preview,
            status_label,
        }
    }

    pub fn begin_preview_load(self: &Rc<Self>, file_path: &str) -> PreviewLoadToken {
        let generation = self.preview_generation.get().wrapping_add(1).max(1);
        self.preview_generation.set(generation);
        let token = PreviewLoadToken(generation);
        self.pending_preview_loading
            .replace(Some(PendingPreviewLoading {
                token,
                file_path: file_path.to_string(),
                message: "Loading file preview...".to_string(),
                kind: PreviewLoadingKind::Provider,
                shown: false,
            }));
        log::debug!(
            "preview load queued file_path={file_path} generation={generation} loading_delay_ms={}",
            PREVIEW_LOADING_DELAY.as_millis()
        );

        let right = Rc::clone(self);
        gtk::glib::timeout_add_local_once(PREVIEW_LOADING_DELAY, move || {
            let loading = {
                let mut pending = right.pending_preview_loading.borrow_mut();
                let Some(loading) = pending.as_mut() else {
                    return;
                };
                if loading.token != token || !right.is_current_load(token) {
                    return;
                }
                loading.shown = true;
                loading.clone()
            };
            log::debug!(
                "preview loading shown file_path={} generation={} delay_ms={}",
                loading.file_path,
                token.0,
                PREVIEW_LOADING_DELAY.as_millis()
            );
            right.show_loading_now(&loading);
        });
        token
    }

    pub fn is_current_load(&self, token: PreviewLoadToken) -> bool {
        self.preview_generation.get() == token.0
    }

    pub fn show_provider_loading(
        &self,
        token: PreviewLoadToken,
        file_path: &str,
        preview_kind: &str,
    ) {
        let message = format!("Loading {preview_kind} preview...");
        self.show_provider_loading_message(token, file_path, &message);
    }

    pub fn show_provider_loading_message(
        &self,
        token: PreviewLoadToken,
        file_path: &str,
        message: &str,
    ) {
        self.configure_loading(token, file_path, message, PreviewLoadingKind::Provider);
    }

    pub fn show_editor_loading(
        &self,
        token: PreviewLoadToken,
        file_path: &str,
        preview_kind: &str,
    ) {
        let message = format!("Loading {preview_kind} preview...");
        self.configure_loading(token, file_path, &message, PreviewLoadingKind::Editor);
    }

    pub fn show_unavailable(&self, file_path: &str, message: &str) {
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

    pub fn show_transfer_in_progress(&self, file_path: &str) {
        self.cancel_preview_load();
        self.set_title(file_path, file_path);
        self.provider_loading_label
            .set_text("File transfer in progress...");
        self.stack.set_visible_child_name("provider-loading");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
        self.status_label.set_text("");
    }

    pub fn show_folder_info(
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

    pub fn show_editor(
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
        self.pending_preview_loading.borrow_mut().take();
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("editor");
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_editor_path.replace(Some(node_path.clone()));
        self.file_editor_disk_signature
            .replace(Some(disk_signature));
        self.file_editor_writable.set(writable);

        let language = crate::ui::file_type::detect(file_path, false).language;
        self.file_editor.set_document_id(language, text);
        self.file_editor.set_editable(writable);
        self.file_editor.set_file_diff(comparison);
        self.file_editor
            .set_markdown_lint_issues(markdown_lint_issues);
        self.file_editor.set_spellcheck_issues(spellcheck_issues);
        self.clear_auxiliary_previews();
        let view_kind = if language == crate::ui::file_type::LanguageId::Csv {
            Some(FileViewKind::Csv)
        } else {
            TextFormat::for_path(file_path).map(FileViewKind::DynamicData)
        };
        if let Some(view_kind) = view_kind {
            self.file_view_button.set_label(match view_kind {
                FileViewKind::Csv => "Table",
                FileViewKind::DynamicData(_) => "View",
            });
            self.file_view_kind.set(Some(view_kind));
            self.file_view_mode_switcher.set_visible(true);
        }
    }

    pub fn show_media_preview(&self, file_path: &str, _subtitle: &str) {
        self.pending_preview_loading.borrow_mut().take();
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("media");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.file_sqlite_preview.clear();
    }

    pub fn show_font_preview(&self, file_path: &str) {
        self.pending_preview_loading.borrow_mut().take();
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("font");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub fn show_sqlite_preview(&self, file_path: &str) {
        self.pending_preview_loading.borrow_mut().take();
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("sqlite");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub fn show_notebook_preview(&self, file_path: &str) {
        self.pending_preview_loading.borrow_mut().take();
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("notebook");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
    }

    pub fn show_pdf_preview(&self, file_path: &str) {
        self.pending_preview_loading.borrow_mut().take();
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("pdf");
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.file_media_preview.clear();
    }

    pub fn show_safetensors_metadata(&self, file_path: &str, metadata_text: &str) {
        self.pending_preview_loading.borrow_mut().take();
        self.set_title(file_path, file_path);
        self.stack.set_visible_child_name("safetensors");
        self.clear_file_state();
        self.file_safetensors_metadata_preview
            .buffer()
            .set_text(metadata_text);
    }

    fn cancel_preview_load(&self) {
        let generation = self.preview_generation.get().wrapping_add(1).max(1);
        self.preview_generation.set(generation);
        self.pending_preview_loading.borrow_mut().take();
    }

    fn configure_loading(
        &self,
        token: PreviewLoadToken,
        file_path: &str,
        message: &str,
        kind: PreviewLoadingKind,
    ) {
        let loading = {
            let mut pending = self.pending_preview_loading.borrow_mut();
            let Some(loading) = pending.as_mut() else {
                return;
            };
            if loading.token != token || !self.is_current_load(token) {
                return;
            }
            loading.file_path = file_path.to_string();
            loading.message = message.to_string();
            loading.kind = kind;
            loading.shown.then(|| loading.clone())
        };
        if let Some(loading) = loading {
            self.show_loading_now(&loading);
        }
    }

    fn show_loading_now(&self, loading: &PendingPreviewLoading) {
        self.set_title(&loading.file_path, &loading.file_path);
        self.clear_file_state();
        self.file_view_split
            .set_start_child(Some(&self.file_editor.root));
        self.file_view_split.set_end_child(None::<&gtk::Widget>);
        self.clear_auxiliary_previews();
        self.status_label.set_text("");

        match loading.kind {
            PreviewLoadingKind::Provider => {
                self.provider_loading_label.set_text(&loading.message);
                self.stack.set_visible_child_name("provider-loading");
            }
            PreviewLoadingKind::Editor => {
                if let Some(label) = self
                    .editor_loading
                    .last_child()
                    .and_then(|child| child.downcast::<gtk::Label>().ok())
                {
                    label.set_text(&loading.message);
                }
                self.file_view_split
                    .set_start_child(Some(&self.editor_loading));
                self.stack.set_visible_child_name("editor");
            }
        }
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
        self.clear_file_view_previews();
    }

    fn clear_auxiliary_previews(&self) {
        self.file_media_preview.clear();
        self.file_sqlite_preview.clear();
        self.clear_file_view_previews();
        self.file_notebook_preview.clear();
        self.file_safetensors_metadata_preview.buffer().set_text("");
    }

    fn clear_file_view_previews(&self) {
        self.file_view_mode_switcher.set_visible(false);
        self.file_code_button.set_active(true);
        self.file_view_kind.set(None);
        self.file_csv_preview.clear();
        self.file_object_preview.clear();
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

pub fn loading_screen_with_label(label: &gtk::Label) -> gtk::Box {
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

    preview.connect_source_offset_changed({
        let editor = editor.clone();
        let preview = Rc::clone(preview);
        let updating_editor = Rc::clone(&updating_editor);
        move || {
            updating_editor.set(true);
            if let Some(offset) = preview.source_offset_at_viewport_top() {
                editor.set_source_offset_at_scroll_top(offset);
            }
            updating_editor.set(false);
        }
    });
}
