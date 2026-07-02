mod ast;
mod blocks;
mod html;
mod parse;
mod render;
mod source_map;
mod style;

use adw::prelude::*;
use gtk::gdk;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use blocks::{MarkdownPreviewBlock, blocks_from_nodes, collect_style_css};
use parse::parse_markdown;
use render::render_document;
use source_map::{
    MarkdownPreviewMeasurement, MarkdownPreviewSourceAnchor, RenderedSourceAnchor,
    measure_source_anchors, source_offset_for_y, y_for_source_offset,
};
use style::install_default_css;

pub(in crate::ui) struct MarkdownPreview {
    pub(in crate::ui) root: gtk::ScrolledWindow,
    document: gtk::Box,
    stylesheet: gtk::CssProvider,
    document_stylesheet: gtk::CssProvider,
    source_anchors: RefCell<Vec<RenderedSourceAnchor>>,
}

pub(in crate::ui) struct MarkdownPreviewDocument {
    blocks: Vec<MarkdownPreviewBlock>,
    stylesheet: String,
}

impl MarkdownPreviewDocument {
    pub(in crate::ui) fn parse(markdown: &str) -> Self {
        let nodes = parse_markdown(markdown);
        let stylesheet = collect_style_css(&nodes);
        let blocks = blocks_from_nodes(&nodes);
        log::debug!(
            "markdown preview document parsed markdown_bytes={} blocks={} stylesheet_bytes={}",
            markdown.len(),
            blocks.len(),
            stylesheet.len(),
        );
        Self { blocks, stylesheet }
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
            .hexpand(true)
            .vexpand(true)
            .child(&document)
            .build();
        root.set_size_request(0, -1);
        root.add_css_class("craic-markdown-preview");

        let stylesheet = gtk::CssProvider::new();
        let document_stylesheet = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &stylesheet,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &document_stylesheet,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }

        Rc::new(Self {
            root,
            document,
            stylesheet,
            document_stylesheet,
            source_anchors: RefCell::new(Vec::new()),
        })
    }

    pub(in crate::ui) fn set_markdown(&self, markdown: &str) {
        self.set_markdown_with_base_path(markdown, None);
    }

    pub(in crate::ui) fn set_markdown_with_base_path(
        &self,
        markdown: &str,
        base_path: Option<&Path>,
    ) {
        self.set_document_with_base_path(MarkdownPreviewDocument::parse(markdown), base_path);
    }

    pub(in crate::ui) fn set_document_with_base_path(
        &self,
        document: MarkdownPreviewDocument,
        base_path: Option<&Path>,
    ) {
        self.document_stylesheet
            .load_from_data(&document.stylesheet);
        let anchors = render_document(
            &self.document,
            &document.blocks,
            base_path.map(Path::to_path_buf),
        );
        self.source_anchors.replace(anchors);
    }

    pub(in crate::ui) fn set_stylesheet(&self, css: &str) {
        self.stylesheet.load_from_data(css);
    }

    pub(in crate::ui) fn measurement(&self) -> MarkdownPreviewMeasurement {
        MarkdownPreviewMeasurement {
            content_height: self.content_height(),
            viewport_height: self.root.allocated_height().max(0),
            source_anchors: self.source_anchors(),
        }
    }

    pub(in crate::ui) fn content_height(&self) -> i32 {
        self.root.vadjustment().upper().round().max(0.0) as i32
    }

    pub(in crate::ui) fn natural_height_for_width(&self, width: i32) -> i32 {
        let (_, natural_height, _, _) = self
            .document
            .measure(gtk::Orientation::Vertical, width.max(0));
        natural_height.max(0)
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
}
