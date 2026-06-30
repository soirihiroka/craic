use super::DragSource;
use gtk::gdk;
use gtk::prelude::*;
use std::rc::Rc;

pub(in crate::ui) const ICON_ROW_HEIGHT: i32 = 34;
pub(in crate::ui) const ICON_ROW_HEIGHT_F64: f64 = ICON_ROW_HEIGHT as f64;
pub(in crate::ui) const ICON_SIZE: i32 = 16;

pub(super) const INDENT_START: i32 = 8;
pub(super) const INDENT_STEP: i32 = 16;
pub(super) const ROW_END_PADDING: i32 = 8;

pub(super) const ROOT_CLASS: &str = "repo-browser-row";
pub(super) const TREE_ROW_CLASS: &str = "repo-browser-tree-row";
pub(super) const BUTTON_CLASS: &str = "repo-browser-row-button";
pub(super) const STICKY_CLASS: &str = "repo-browser-sticky-row";
pub(super) const BOTTOM_STICKY_CLASS: &str = "repo-browser-sticky-bottom";
pub(super) const DROP_TARGET_CLASS: &str = "repo-browser-drop-target";
pub(super) const DIMMED_CLASS: &str = "repo-browser-ignored-content";
pub(super) const DISCLOSURE_CLASS: &str = "repo-browser-disclosure";
pub(super) const CONTENT_CLASS: &str = "craic-tree-icon-row-content";
pub(super) const INDENT_CLASS: &str = "craic-tree-icon-row-indent";
pub(super) const ICON_CLASS: &str = "craic-tree-icon-row-icon";
pub(super) const TITLE_CLASS: &str = "craic-tree-icon-row-title";
pub(super) const ENTRY_CLASS: &str = "craic-tree-icon-row-entry";
const PROGRESS_CLASS: &str = "craic-tree-icon-row-progress";
const PROGRESS_HOVER_CLASS: &str = "craic-tree-icon-row-progress-hover";

pub(in crate::ui) type IconRowProgressCallback = Rc<dyn Fn()>;
type EntryCallback = Rc<dyn Fn(String)>;
type VoidCallback = Rc<dyn Fn()>;
type ChangedCallback = Rc<dyn Fn(&gtk::Entry, &gtk::Widget)>;
type ClickCallback = Rc<dyn Fn(&gtk::Button, &gtk::GestureClick, f64, f64)>;

#[derive(Clone, PartialEq)]
pub(in crate::ui) struct IconRowProgress {
    pub(in crate::ui) fraction: f64,
    pub(in crate::ui) tooltip: String,
}

pub(in crate::ui) struct IconRow {
    pub(in crate::ui) root: gtk::Box,
    pub(in crate::ui) title: gtk::Label,
}

impl IconRow {
    pub(in crate::ui) fn builder(title: impl Into<String>) -> IconRowBuilder {
        IconRowBuilder {
            title: title.into(),
            depth: 0,
            height: ICON_ROW_HEIGHT,
            selected: false,
            sticky: false,
            bottom_sticky: false,
            disclosure: None,
            icon: None,
            end_padding: ROW_END_PADDING,
            progress: None,
            dimmed: false,
            edit: None,
            on_primary_click: None,
            on_secondary_click: None,
            drag_source: None,
            trailing: Vec::new(),
        }
    }
}

pub(in crate::ui) struct IconRowBuilder {
    title: String,
    depth: usize,
    height: i32,
    selected: bool,
    sticky: bool,
    bottom_sticky: bool,
    disclosure: Option<gtk::Widget>,
    icon: Option<gtk::Widget>,
    end_padding: i32,
    progress: Option<(IconRowProgress, IconRowProgressCallback)>,
    dimmed: bool,
    edit: Option<IconRowEdit>,
    on_primary_click: Option<ClickCallback>,
    on_secondary_click: Option<ClickCallback>,
    drag_source: Option<DragSource>,
    trailing: Vec<gtk::Widget>,
}

#[derive(Default)]
struct IconRowEdit {
    on_activate: Option<EntryCallback>,
    on_escape: Option<VoidCallback>,
    on_focus_leave: Option<VoidCallback>,
    on_changed: Option<ChangedCallback>,
}

impl IconRowBuilder {
    pub(in crate::ui) fn set_icon<W>(mut self, icon: W) -> Self
    where
        W: IsA<gtk::Widget>,
    {
        self.icon = Some(icon.upcast());
        self
    }

    pub(in crate::ui) fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    pub(in crate::ui) fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub(in crate::ui) fn sticky(mut self, sticky: bool) -> Self {
        self.sticky = sticky;
        self
    }

    pub(in crate::ui) fn bottom_sticky(mut self, bottom_sticky: bool) -> Self {
        self.bottom_sticky = bottom_sticky;
        self
    }

    pub(in crate::ui) fn disclosure<W>(mut self, disclosure: W) -> Self
    where
        W: IsA<gtk::Widget>,
    {
        self.disclosure = Some(disclosure.upcast());
        self
    }

    pub(in crate::ui) fn end_padding(mut self, end_padding: i32) -> Self {
        self.end_padding = end_padding;
        self
    }

    pub(in crate::ui) fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }

    pub(in crate::ui) fn editable(mut self) -> Self {
        self.edit.get_or_insert_with(IconRowEdit::default);
        self
    }

    pub(in crate::ui) fn on_edit_activate<F>(mut self, callback: F) -> Self
    where
        F: Fn(String) + 'static,
    {
        self.edit
            .get_or_insert_with(IconRowEdit::default)
            .on_activate = Some(Rc::new(callback));
        self
    }

    pub(in crate::ui) fn on_edit_escape<F>(mut self, callback: F) -> Self
    where
        F: Fn() + 'static,
    {
        self.edit.get_or_insert_with(IconRowEdit::default).on_escape = Some(Rc::new(callback));
        self
    }

    pub(in crate::ui) fn on_edit_focus_leave<F>(mut self, callback: F) -> Self
    where
        F: Fn() + 'static,
    {
        self.edit
            .get_or_insert_with(IconRowEdit::default)
            .on_focus_leave = Some(Rc::new(callback));
        self
    }

    pub(in crate::ui) fn on_edit_changed<F>(mut self, callback: F) -> Self
    where
        F: Fn(&gtk::Entry, &gtk::Widget) + 'static,
    {
        self.edit
            .get_or_insert_with(IconRowEdit::default)
            .on_changed = Some(Rc::new(callback));
        self
    }

    pub(in crate::ui) fn progress<F>(mut self, progress: IconRowProgress, on_activate: F) -> Self
    where
        F: Fn() + 'static,
    {
        self.progress = Some((progress, Rc::new(on_activate)));
        self
    }

    pub(in crate::ui) fn on_primary_click<F>(mut self, callback: F) -> Self
    where
        F: Fn(&gtk::Button, &gtk::GestureClick, f64, f64) + 'static,
    {
        self.on_primary_click = Some(Rc::new(callback));
        self
    }

    pub(in crate::ui) fn on_secondary_click<F>(mut self, callback: F) -> Self
    where
        F: Fn(&gtk::Button, &gtk::GestureClick, f64, f64) + 'static,
    {
        self.on_secondary_click = Some(Rc::new(callback));
        self
    }

    pub(in crate::ui) fn drag_source(mut self, drag_source: DragSource) -> Self {
        self.drag_source = Some(drag_source);
        self
    }

    pub(in crate::ui) fn trailing<W>(mut self, trailing: W) -> Self
    where
        W: IsA<gtk::Widget>,
    {
        self.trailing.push(trailing.upcast());
        self
    }

    pub(in crate::ui) fn build(self) -> IconRow {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .height_request(self.height)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .spacing(7)
            .build();
        root.add_css_class(ROOT_CLASS);
        root.add_css_class(TREE_ROW_CLASS);
        if self.sticky {
            root.add_css_class(STICKY_CLASS);
        }
        if self.bottom_sticky {
            root.add_css_class(BOTTOM_STICKY_CLASS);
        }
        if self.selected {
            root.add_css_class("selected");
        }

        let editing = self.edit.is_some();
        let button = gtk::Button::builder()
            .height_request(self.height)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .build();
        if !self.selected || editing {
            button.add_css_class("flat");
        }
        button.add_css_class(BUTTON_CLASS);
        install_click_handlers(
            &button,
            self.on_primary_click.clone(),
            self.on_secondary_click.clone(),
        );
        if let Some(drag_source) = &self.drag_source {
            drag_source.install(&button);
        }

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .height_request(self.height)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .spacing(7)
            .margin_end(self.end_padding)
            .build();
        content.add_css_class(CONTENT_CLASS);
        if editing {
            root.append(&content);
        } else {
            button.set_child(Some(&content));
            root.append(&button);
        }

        let indent = indent_widget(self.depth);
        content.append(&indent);
        if let Some(disclosure) = self.disclosure {
            disclosure.add_css_class(DISCLOSURE_CLASS);
            sync_dimmed(&disclosure, self.dimmed);
            content.append(&disclosure);
        }
        if let Some(icon) = &self.icon {
            icon.add_css_class(ICON_CLASS);
            sync_dimmed(icon, self.dimmed);
            content.append(icon);
        }

        let title = gtk::Label::builder()
            .label(&self.title)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        title.add_css_class(TITLE_CLASS);
        sync_dimmed(&title, self.dimmed);

        if let Some(edit) = self.edit {
            let row_entry = gtk::Entry::builder()
                .text(&self.title)
                .hexpand(true)
                .valign(gtk::Align::Center)
                .build();
            row_entry.add_css_class(ENTRY_CLASS);
            content.append(&row_entry);

            if let Some(callback) = edit.on_changed {
                let callback_widget = self
                    .icon
                    .clone()
                    .unwrap_or_else(|| row_entry.clone().upcast::<gtk::Widget>());
                row_entry.connect_changed(move |entry| callback(entry, &callback_widget));
            }

            if let Some(callback) = edit.on_activate {
                row_entry.connect_activate(move |entry| callback(entry.text().trim().to_string()));
            }

            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed({
                let on_escape = edit.on_escape.clone();

                move |_, key, _, _| match key {
                    gdk::Key::Escape => {
                        if let Some(callback) = &on_escape {
                            callback();
                        }
                        gtk::glib::Propagation::Stop
                    }
                    gdk::Key::Up | gdk::Key::Down => gtk::glib::Propagation::Stop,
                    _ => gtk::glib::Propagation::Proceed,
                }
            });
            row_entry.add_controller(keys);

            if let Some(callback) = edit.on_focus_leave {
                let focus = gtk::EventControllerFocus::new();
                focus.connect_leave(move |_| callback());
                row_entry.add_controller(focus);
            }
        } else {
            content.append(&title);
            if let Some((progress, on_activate)) = self.progress {
                let widget = progress_widget(&progress, on_activate);
                content.append(&widget);
            }
            for trailing in self.trailing {
                content.append(&trailing);
            }
        }

        IconRow { root, title }
    }
}

pub(super) fn indent_widget(depth: usize) -> gtk::Box {
    let indent = gtk::Box::builder()
        .width_request(indent_width(depth))
        .build();
    indent.add_css_class(INDENT_CLASS);
    indent
}

pub(super) fn indent_width(depth: usize) -> i32 {
    INDENT_START + depth as i32 * INDENT_STEP
}

pub(in crate::ui) fn sync_icon_row_depth(widget: &gtk::Widget, depth: usize) -> bool {
    let Some(indent) = child_with_class(widget, INDENT_CLASS) else {
        return false;
    };
    let width = indent_width(depth);
    if indent.width_request() == width {
        return false;
    }
    indent.set_width_request(width);
    true
}

pub(in crate::ui) fn sync_icon_row_selected(widget: &gtk::Widget, selected: bool) -> bool {
    let mut changed = false;
    if selected && !widget.has_css_class("selected") {
        widget.add_css_class("selected");
        changed = true;
    } else if !selected && widget.has_css_class("selected") {
        widget.remove_css_class("selected");
        changed = true;
    }

    if let Some(button) = widget
        .first_child()
        .and_then(|child| child.downcast::<gtk::Button>().ok())
    {
        if selected && button.has_css_class("flat") {
            button.remove_css_class("flat");
            changed = true;
        } else if !selected && !button.has_css_class("flat") {
            button.add_css_class("flat");
            changed = true;
        }
    }
    changed
}

pub(in crate::ui) fn sync_icon_row_bottom_sticky(widget: &impl IsA<gtk::Widget>, bottom: bool) {
    if bottom && !widget.has_css_class(BOTTOM_STICKY_CLASS) {
        widget.add_css_class(BOTTOM_STICKY_CLASS);
    } else if !bottom && widget.has_css_class(BOTTOM_STICKY_CLASS) {
        widget.remove_css_class(BOTTOM_STICKY_CLASS);
    }
}

pub(in crate::ui) fn sync_icon_row_drop_target(widget: &impl IsA<gtk::Widget>, drop_target: bool) {
    if drop_target && !widget.has_css_class(DROP_TARGET_CLASS) {
        widget.add_css_class(DROP_TARGET_CLASS);
    } else if !drop_target && widget.has_css_class(DROP_TARGET_CLASS) {
        widget.remove_css_class(DROP_TARGET_CLASS);
    }
}

pub(in crate::ui) fn sync_dimmed(widget: &impl IsA<gtk::Widget>, dimmed: bool) -> bool {
    if dimmed && !widget.has_css_class(DIMMED_CLASS) {
        widget.add_css_class(DIMMED_CLASS);
        true
    } else if !dimmed && widget.has_css_class(DIMMED_CLASS) {
        widget.remove_css_class(DIMMED_CLASS);
        true
    } else {
        false
    }
}

pub(in crate::ui) fn icon_row_content(widget: &gtk::Widget) -> Option<gtk::Box> {
    child_with_class(widget, CONTENT_CLASS).and_then(|child| child.downcast::<gtk::Box>().ok())
}

pub(in crate::ui) fn icon_row_disclosure(widget: &gtk::Widget) -> Option<gtk::DrawingArea> {
    child_with_class(widget, DISCLOSURE_CLASS)
        .and_then(|child| child.downcast::<gtk::DrawingArea>().ok())
}

pub(in crate::ui) fn icon_row_icon(widget: &gtk::Widget) -> Option<gtk::Widget> {
    child_with_class(widget, ICON_CLASS)
}

pub(in crate::ui) fn icon_row_title(widget: &gtk::Widget) -> Option<gtk::Label> {
    child_with_class(widget, TITLE_CLASS).and_then(|child| child.downcast::<gtk::Label>().ok())
}

pub(in crate::ui) fn icon_row_entry(widget: gtk::Widget) -> Option<gtk::Entry> {
    child_with_class(&widget, ENTRY_CLASS).and_then(|child| child.downcast::<gtk::Entry>().ok())
}

pub(in crate::ui) fn sync_icon_row_text(widget: &gtk::Widget, text: &str) {
    let Some(entry) = icon_row_entry(widget.clone()) else {
        return;
    };
    if entry.text().as_str() == text {
        return;
    }
    entry.set_text(text);
    entry.set_position(0);
}

pub(in crate::ui) fn icon_row_child_after(
    anchor: &impl IsA<gtk::Widget>,
    class_name: &str,
) -> Option<gtk::Widget> {
    let mut child = anchor.next_sibling();
    while let Some(widget) = child {
        if widget.has_css_class(class_name) {
            return Some(widget);
        }
        child = widget.next_sibling();
    }
    None
}

pub(in crate::ui) fn sync_icon_row_progress(
    widget: &gtk::Widget,
    progress: Option<&IconRowProgress>,
    on_activate: Option<IconRowProgressCallback>,
) {
    let Some(content) = icon_row_content(widget) else {
        return;
    };
    let Some(title) = icon_row_title(widget) else {
        return;
    };

    if let Some(existing) = icon_row_child_after(&title, PROGRESS_CLASS) {
        if let Some(progress) = progress
            && let Ok(area) = existing.clone().downcast::<gtk::DrawingArea>()
        {
            update_progress_widget(&area, progress);
            return;
        }

        content.remove(&existing);
    }

    if let (Some(progress), Some(on_activate)) = (progress, on_activate) {
        let widget = progress_widget(progress, on_activate);
        content.insert_child_after(&widget, Some(&title));
    }
}

fn child_with_class(widget: &gtk::Widget, class_name: &str) -> Option<gtk::Widget> {
    if widget.has_css_class(class_name) {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = child_with_class(&current, class_name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn progress_widget(
    progress: &IconRowProgress,
    on_activate: IconRowProgressCallback,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(24)
        .content_height(22)
        .width_request(24)
        .height_request(22)
        .valign(gtk::Align::Center)
        .build();
    area.add_css_class(PROGRESS_CLASS);

    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter({
        let area = area.clone();

        move |_, _, _| {
            area.add_css_class(PROGRESS_HOVER_CLASS);
            area.queue_draw();
        }
    });
    motion.connect_leave({
        let area = area.clone();

        move |_| {
            area.remove_css_class(PROGRESS_HOVER_CLASS);
            area.queue_draw();
        }
    });
    area.add_controller(motion);

    let click = gtk::GestureClick::builder().button(0).build();
    click.connect_released(move |_, _, _, _| on_activate());
    area.add_controller(click);

    update_progress_widget(&area, progress);
    area
}

fn install_click_handlers(
    button: &gtk::Button,
    on_primary_click: Option<ClickCallback>,
    on_secondary_click: Option<ClickCallback>,
) {
    if on_primary_click.is_none() && on_secondary_click.is_none() {
        return;
    }

    let click = gtk::GestureClick::builder().button(0).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let button = button.clone();

        move |gesture, _, x, y| match gesture.current_button() {
            1 => {
                if let Some(callback) = &on_primary_click {
                    callback(&button, gesture, x, y);
                }
            }
            3 => {
                if let Some(callback) = &on_secondary_click {
                    callback(&button, gesture, x, y);
                }
            }
            _ => {}
        }
    });
    button.add_controller(click);
}

fn update_progress_widget(area: &gtk::DrawingArea, progress: &IconRowProgress) {
    let progress = progress.clone();
    area.set_tooltip_text(Some(&format!("{} - click to cancel", progress.tooltip)));
    area.set_draw_func(move |area, context, width, height| {
        let hovered = area.has_css_class(PROGRESS_HOVER_CLASS);
        draw_progress(area, context, width, height, &progress, hovered);
    });
    area.queue_draw();
}

fn draw_progress(
    area: &gtk::DrawingArea,
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    progress: &IconRowProgress,
    hovered: bool,
) {
    let color = area.color();
    let width = width as f64;
    let height = height as f64;
    let center_x = width / 2.0;
    let center_y = height / 2.0;
    let radius = (height.min(width) / 2.0 - 2.5).max(1.0);
    let start = -std::f64::consts::FRAC_PI_2;
    let end = start + progress.fraction.clamp(0.0, 1.0) * std::f64::consts::TAU;

    context.set_line_width(2.4);
    context.set_source_rgba(
        color.red() as f64,
        color.green() as f64,
        color.blue() as f64,
        (color.alpha() * 0.22) as f64,
    );
    context.arc(center_x, center_y, radius, 0.0, std::f64::consts::TAU);
    let _ = context.stroke();

    context.set_source_rgba(
        color.red() as f64,
        color.green() as f64,
        color.blue() as f64,
        color.alpha() as f64,
    );
    context.arc(center_x, center_y, radius, start, end);
    let _ = context.stroke();

    if hovered {
        context.set_line_width(2.0);
        let half = radius * 0.42;
        context.move_to(center_x - half, center_y - half);
        context.line_to(center_x + half, center_y + half);
        context.move_to(center_x + half, center_y - half);
        context.line_to(center_x - half, center_y + half);
        let _ = context.stroke();
    }
}
