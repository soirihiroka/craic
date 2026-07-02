mod ast;
mod blocks;
mod html;
mod parse;
mod render;
mod source_map;
mod style;

use adw::prelude::*;
use gtk::{gdk, pango};
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
    selected_text: RefCell<Option<String>>,
    selection_drag_anchor: RefCell<Option<SelectionPoint>>,
    whole_selection_active: Cell<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectionPoint {
    widget_index: usize,
    offset: i32,
}

#[derive(Clone)]
enum SelectableTextWidget {
    Label(gtk::Label),
    TextView(gtk::TextView),
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
            selected_text: RefCell::new(None),
            selection_drag_anchor: RefCell::new(None),
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
        self.selected_text.replace(None);
        self.selection_drag_anchor.replace(None);
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
        self.selected_text.replace(None);
        self.whole_selection_active.set(true);
    }

    fn copy_preview_selection(&self) -> bool {
        if self.whole_selection_active.get() {
            let text = self.rendered_text.borrow();
            if text.is_empty() {
                return false;
            }
            self.root.clipboard().set_text(&text);
            return true;
        }

        let Some(text) = self.selected_text.borrow().clone() else {
            return false;
        };
        if text.is_empty() {
            return false;
        }
        self.root.clipboard().set_text(&text);
        true
    }

    fn clear_preview_selection(&self) {
        clear_text_widget_selection(self.document.upcast_ref());
        self.selected_text.replace(None);
        self.selection_drag_anchor.replace(None);
        self.whole_selection_active.set(false);
    }

    fn begin_preview_selection(&self, x: f64, y: f64) {
        let anchor = selection_point_at_document_position(&self.document, x, y);
        self.selected_text.replace(None);
        self.whole_selection_active.set(false);
        self.selection_drag_anchor.replace(anchor);
    }

    fn update_preview_selection(&self, x: f64, y: f64) {
        let Some(anchor) = *self.selection_drag_anchor.borrow() else {
            return;
        };
        let Some(focus) = selection_point_at_document_position(&self.document, x, y) else {
            return;
        };
        let selected_text = apply_text_widget_selection(&self.document, anchor, focus);
        self.selected_text.replace(selected_text);
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
                && preview.copy_preview_selection()
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
                preview.clear_preview_selection();
            }
        }
    });
    preview.document.add_controller(clicks);

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    drag.connect_drag_begin({
        let preview = Rc::downgrade(preview);
        move |_, x, y| {
            if let Some(preview) = preview.upgrade() {
                preview.begin_preview_selection(x, y);
            }
        }
    });
    drag.connect_drag_update({
        let preview = Rc::downgrade(preview);
        move |gesture, offset_x, offset_y| {
            let Some(preview) = preview.upgrade() else {
                return;
            };
            let Some((start_x, start_y)) = gesture.start_point() else {
                return;
            };
            preview.update_preview_selection(start_x + offset_x, start_y + offset_y);
        }
    });
    drag.connect_drag_end({
        let preview = Rc::downgrade(preview);
        move |gesture, offset_x, offset_y| {
            let Some(preview) = preview.upgrade() else {
                return;
            };
            if let Some((start_x, start_y)) = gesture.start_point() {
                preview.update_preview_selection(start_x + offset_x, start_y + offset_y);
            }
            preview.selection_drag_anchor.replace(None);
        }
    });
    preview.document.add_controller(drag);
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

fn clear_text_widget_selection(widget: &gtk::Widget) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        label.select_region(0, 0);
    }
    if let Some(text_view) = widget.downcast_ref::<gtk::TextView>() {
        let buffer = text_view.buffer();
        let start = buffer.start_iter();
        buffer.select_range(&start, &start);
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        clear_text_widget_selection(&current);
        child = current.next_sibling();
    }
}

fn selection_point_at_document_position(
    document: &gtk::Box,
    x: f64,
    y: f64,
) -> Option<SelectionPoint> {
    let widgets = selectable_text_widgets(document.upcast_ref());
    let mut previous_end = None;

    for (index, widget) in widgets.iter().enumerate() {
        let Some((_, top)) = widget.widget().translate_coordinates(document, 0.0, 0.0) else {
            continue;
        };
        let bottom = top + f64::from(widget.widget().allocated_height()).max(1.0);

        if y < top {
            return previous_end.or(Some(SelectionPoint {
                widget_index: index,
                offset: 0,
            }));
        }

        if y <= bottom {
            let (left, top) = widget.widget().translate_coordinates(document, 0.0, 0.0)?;
            return Some(SelectionPoint {
                widget_index: index,
                offset: widget.offset_at(x - left, y - top),
            });
        }

        previous_end = Some(SelectionPoint {
            widget_index: index,
            offset: widget.text_len(),
        });
    }

    previous_end
}

fn apply_text_widget_selection(
    document: &gtk::Box,
    anchor: SelectionPoint,
    focus: SelectionPoint,
) -> Option<String> {
    let widgets = selectable_text_widgets(document.upcast_ref());
    if anchor.widget_index >= widgets.len() || focus.widget_index >= widgets.len() {
        return None;
    }

    let (start, end) = if (focus.widget_index, focus.offset) < (anchor.widget_index, anchor.offset)
    {
        (focus, anchor)
    } else {
        (anchor, focus)
    };

    if start == end {
        for widget in &widgets {
            widget.clear_selection();
        }
        return None;
    }

    let mut parts = Vec::new();
    for (index, widget) in widgets.iter().enumerate() {
        if index < start.widget_index || index > end.widget_index {
            widget.clear_selection();
            continue;
        }

        let start_offset = if index == start.widget_index {
            start.offset
        } else {
            0
        };
        let end_offset = if index == end.widget_index {
            end.offset
        } else {
            widget.text_len()
        };

        if start_offset == end_offset {
            widget.clear_selection();
            continue;
        }

        let selected = widget.select_range(start_offset, end_offset);
        if !selected.is_empty() {
            parts.push(selected);
        }
    }

    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn selectable_text_widgets(widget: &gtk::Widget) -> Vec<SelectableTextWidget> {
    let mut widgets = Vec::new();
    collect_selectable_text_widgets(widget, &mut widgets);
    widgets
}

fn collect_selectable_text_widgets(widget: &gtk::Widget, output: &mut Vec<SelectableTextWidget>) {
    if !widget.is_visible() {
        return;
    }

    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        if label.is_selectable() && !label.text().is_empty() {
            output.push(SelectableTextWidget::Label(label.clone()));
        }
        return;
    }

    if let Some(text_view) = widget.downcast_ref::<gtk::TextView>() {
        if text_view.buffer().char_count() > 0 {
            output.push(SelectableTextWidget::TextView(text_view.clone()));
        }
        return;
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        collect_selectable_text_widgets(&current, output);
        child = current.next_sibling();
    }
}

impl SelectableTextWidget {
    fn widget(&self) -> gtk::Widget {
        match self {
            Self::Label(label) => label.clone().upcast(),
            Self::TextView(text_view) => text_view.clone().upcast(),
        }
    }

    fn text_len(&self) -> i32 {
        match self {
            Self::Label(label) => label.text().chars().count() as i32,
            Self::TextView(text_view) => text_view.buffer().char_count(),
        }
    }

    fn offset_at(&self, x: f64, y: f64) -> i32 {
        match self {
            Self::Label(label) => label_offset_at(label, x, y),
            Self::TextView(text_view) => text_view_offset_at(text_view, x, y),
        }
    }

    fn clear_selection(&self) {
        match self {
            Self::Label(label) => label.select_region(0, 0),
            Self::TextView(text_view) => {
                let buffer = text_view.buffer();
                let start = buffer.start_iter();
                buffer.select_range(&start, &start);
            }
        }
    }

    fn select_range(&self, start: i32, end: i32) -> String {
        let start = start.clamp(0, self.text_len());
        let end = end.clamp(0, self.text_len());
        match self {
            Self::Label(label) => {
                label.select_region(start, end);
                label
                    .text()
                    .chars()
                    .skip(start as usize)
                    .take(end.saturating_sub(start) as usize)
                    .collect()
            }
            Self::TextView(text_view) => {
                let buffer = text_view.buffer();
                let start_iter = buffer.iter_at_offset(start);
                let end_iter = buffer.iter_at_offset(end);
                buffer.select_range(&start_iter, &end_iter);
                buffer.text(&start_iter, &end_iter, true).to_string()
            }
        }
    }
}

fn label_offset_at(label: &gtk::Label, x: f64, y: f64) -> i32 {
    let text = label.text();
    let text = text.as_str();
    let text_len = text.chars().count() as i32;
    if text_len == 0 {
        return 0;
    }

    if y < 0.0 {
        return 0;
    }
    if y > f64::from(label.allocated_height()) {
        return text_len;
    }

    let (layout_x, layout_y) = label.layout_offsets();
    let layout = label.layout();
    let pango_x = ((x.round() as i32 - layout_x) * pango::SCALE).clamp(i32::MIN / 2, i32::MAX / 2);
    let pango_y = ((y.round() as i32 - layout_y) * pango::SCALE).clamp(i32::MIN / 2, i32::MAX / 2);
    let (_, byte_index, trailing) = layout.xy_to_index(pango_x, pango_y);
    byte_index_to_char_offset(text, byte_index, trailing).clamp(0, text_len)
}

fn text_view_offset_at(text_view: &gtk::TextView, x: f64, y: f64) -> i32 {
    let buffer = text_view.buffer();
    if y < 0.0 {
        return 0;
    }
    if y > f64::from(text_view.allocated_height()) {
        return buffer.char_count();
    }

    let (buffer_x, buffer_y) = text_view.window_to_buffer_coords(
        gtk::TextWindowType::Widget,
        x.round() as i32,
        y.round() as i32,
    );
    text_view
        .iter_at_location(buffer_x, buffer_y)
        .map(|iter| iter.offset())
        .unwrap_or_else(|| buffer.char_count())
}

fn byte_index_to_char_offset(text: &str, byte_index: i32, trailing: i32) -> i32 {
    let byte_index = (byte_index.max(0) as usize).min(text.len());
    let byte_index = if text.is_char_boundary(byte_index) {
        byte_index
    } else {
        text.char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index < byte_index)
            .last()
            .unwrap_or(0)
    };

    text[..byte_index].chars().count() as i32 + trailing.max(0)
}
