mod ast;
mod blocks;
mod html;
mod parse;
mod render;
mod source_map;
mod style;

use adw::prelude::*;
use gtk::gdk;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use blocks::{MarkdownPreviewBlock, block_text, blocks_from_nodes, collect_style_css};
use parse::parse_markdown;
use render::render_document;
use source_map::{
    MarkdownPreviewSourceAnchor, RenderedSourceAnchor, measure_source_anchors, source_offset_for_y,
    y_for_source_offset,
};
use style::install_default_css;

pub(in crate::ui) struct MarkdownPreview {
    pub(in crate::ui) root: gtk::ScrolledWindow,
    document: gtk::Box,
    document_stylesheet: gtk::CssProvider,
    source_anchors: RefCell<Vec<RenderedSourceAnchor>>,
    rendered_text: RefCell<String>,
    whole_selection_active: Cell<bool>,
}

pub(in crate::ui) struct MarkdownPreviewDocument {
    blocks: Vec<MarkdownPreviewBlock>,
    stylesheet: String,
    plain_text: String,
}

impl MarkdownPreviewDocument {
    pub(in crate::ui) fn parse(markdown: &str) -> Self {
        let nodes = parse_markdown(markdown);
        let stylesheet = collect_style_css(&nodes);
        let blocks = blocks_from_nodes(&nodes);
        let plain_text = block_text(&blocks).plain_text;
        log::debug!(
            "markdown preview document parsed markdown_bytes={} blocks={} stylesheet_bytes={} plain_text_bytes={}",
            markdown.len(),
            blocks.len(),
            stylesheet.len(),
            plain_text.len(),
        );
        Self {
            blocks,
            stylesheet,
            plain_text,
        }
    }
}

impl MarkdownPreview {
    pub(in crate::ui) fn new() -> Rc<Self> {
        install_default_css();

        let document = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Start)
            .build();
        document.set_margin_top(24);
        document.set_margin_bottom(24);
        document.set_margin_start(24);
        document.set_margin_end(24);
        document.set_size_request(0, -1);
        document.add_css_class("craic-markdown-document");

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_width(0)
            .propagate_natural_width(false)
            .focusable(true)
            .hexpand(true)
            .vexpand(true)
            .child(&document)
            .build();
        root.set_size_request(0, -1);
        root.add_css_class("craic-markdown-preview");

        let document_stylesheet = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &document_stylesheet,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }

        let preview = Rc::new(Self {
            root,
            document,
            document_stylesheet,
            source_anchors: RefCell::new(Vec::new()),
            rendered_text: RefCell::new(String::new()),
            whole_selection_active: Cell::new(false),
        });
        install_preview_selection_handlers(&preview);
        preview
    }

    pub(in crate::ui) fn set_document_with_base_path(
        &self,
        document: MarkdownPreviewDocument,
        base_path: Option<&Path>,
    ) {
        self.document_stylesheet
            .load_from_data(&document.stylesheet);
        self.rendered_text.replace(document.plain_text.clone());
        self.whole_selection_active.set(false);
        let anchors = render_document(
            &self.document,
            &document.blocks,
            base_path.map(Path::to_path_buf),
        );
        self.source_anchors.replace(anchors);
    }

    pub(in crate::ui) fn source_anchors(&self) -> Vec<MarkdownPreviewSourceAnchor> {
        measure_source_anchors(&self.document, &self.source_anchors.borrow())
    }

    pub(in crate::ui) fn source_offset_at_y(&self, y: f64) -> Option<usize> {
        source_offset_for_y(&self.source_anchors(), y)
    }

    pub(in crate::ui) fn y_for_source_offset(&self, source_offset: usize) -> Option<f64> {
        y_for_source_offset(&self.source_anchors(), source_offset)
    }

    pub(in crate::ui) fn source_offset_at_viewport_top(&self) -> Option<usize> {
        self.source_offset_at_y(self.root.vadjustment().value())
    }

    pub(in crate::ui) fn scroll_to_source_offset(&self, source_offset: usize) -> bool {
        let Some(y) = self.y_for_source_offset(source_offset) else {
            return false;
        };

        let adjustment = self.root.vadjustment();
        let max = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value(y.clamp(adjustment.lower(), max));
        true
    }

    fn select_all_rendered_text(&self) {
        select_all_text_widgets(self.document.upcast_ref());
        self.whole_selection_active.set(true);
    }

    fn copy_whole_selection(&self) -> bool {
        if !self.whole_selection_active.get() {
            return false;
        }

        let text = self.rendered_text.borrow();
        if text.is_empty() {
            return false;
        }
        self.root.clipboard().set_text(&text);
        true
    }
}

fn install_preview_selection_handlers(preview: &Rc<MarkdownPreview>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let preview = Rc::downgrade(preview);
        move |_, key, _, modifiers| {
            let Some(preview) = preview.upgrade() else {
                return gtk::glib::Propagation::Proceed;
            };
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
            if ctrl && !alt && matches!(key, gdk::Key::a | gdk::Key::A) {
                preview.select_all_rendered_text();
                return gtk::glib::Propagation::Stop;
            }
            if ctrl
                && !alt
                && matches!(key, gdk::Key::c | gdk::Key::C)
                && preview.copy_whole_selection()
            {
                return gtk::glib::Propagation::Stop;
            }

            gtk::glib::Propagation::Proceed
        }
    });
    preview.root.add_controller(keys);

    let clicks = gtk::GestureClick::new();
    clicks.set_propagation_phase(gtk::PropagationPhase::Capture);
    clicks.connect_pressed({
        let preview = Rc::downgrade(preview);
        move |_, _, _, _| {
            if let Some(preview) = preview.upgrade() {
                preview.whole_selection_active.set(false);
            }
        }
    });
    preview.document.add_controller(clicks);
}

fn select_all_text_widgets(widget: &gtk::Widget) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        label.select_region(0, -1);
    }
    if let Some(text_view) = widget.downcast_ref::<gtk::TextView>() {
        let buffer = text_view.buffer();
        let (start, end) = buffer.bounds();
        buffer.select_range(&start, &end);
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        select_all_text_widgets(&current);
        child = current.next_sibling();
    }
}
