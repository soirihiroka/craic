use super::{
    DisclosureAnimator, ICON_SIZE, TreeRenderState, TreeRow, icon_row_entry, sticky_items,
};
use crate::reconcile::{Element, PartialEqRenderState};
use crate::ui::{canvas_scroll, canvas_scrollbar};
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::hash::Hash;
use std::rc::Rc;
use std::sync::OnceLock;

type MountFn<K, S> = dyn Fn(usize, &K, &TreeRenderState<K, S>) -> gtk::Widget;
type UpdateFn<K, S> = dyn Fn(usize, &gtk::Widget, &TreeRenderState<K, S>, &TreeRenderState<K, S>);
type PointerPressFn<K, S> = dyn Fn(&gtk::GestureClick, f64, f64, f64, Option<TreeRow<K, S>>);

pub struct TreeRenderer<K, S> {
    mount: Rc<MountFn<K, S>>,
    update: Rc<UpdateFn<K, S>>,
}

impl<K, S> Clone for TreeRenderer<K, S> {
    fn clone(&self) -> Self {
        Self {
            mount: self.mount.clone(),
            update: self.update.clone(),
        }
    }
}

impl<K, S> TreeRenderer<K, S> {
    pub fn new<M, U>(mount: M, update: U) -> Self
    where
        M: Fn(usize, &K, &TreeRenderState<K, S>) -> gtk::Widget + 'static,
        U: Fn(usize, &gtk::Widget, &TreeRenderState<K, S>, &TreeRenderState<K, S>) + 'static,
    {
        Self {
            mount: Rc::new(mount),
            update: Rc::new(update),
        }
    }
}

static CSS_INSTALLED: OnceLock<()> = OnceLock::new();

fn install_css() {
    CSS_INSTALLED.get_or_init(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(
            r#"
            .craic-tree-view .repo-browser-list {
                background-color: @window_bg_color;
            }
            .craic-tree-view .repo-browser-row {
                background-color: @window_bg_color;
                color: @view_fg_color;
            }
            .craic-tree-view .repo-browser-row-button {
                padding: 0 8px 0 0;
                border-radius: 0;
            }
            .craic-tree-view .repo-browser-row.selected {
                color: @view_fg_color;
            }
            .craic-tree-view .repo-browser-row.repo-browser-drop-target {
                background-color: alpha(@accent_bg_color, 0.16);
            }
            .craic-tree-view .repo-browser-tree-row {
                font-size: 0.95em;
            }
            .craic-tree-view .repo-browser-disclosure {
                min-width: 16px;
                min-height: 16px;
            }
            .craic-tree-view .repo-browser-search-row {
                padding: 0 8px 0 10px;
            }
            .craic-tree-view .repo-browser-status-row {
                padding-left: 10px;
            }
            .craic-tree-view .repo-browser-search-option {
                min-width: 32px;
                min-height: 32px;
                padding: 0 4px;
            }
            .craic-tree-view .repo-browser-row.repo-browser-sticky-row,
            .craic-tree-view .repo-browser-row.repo-browser-sticky-row:hover,
            .craic-tree-view .repo-browser-row.repo-browser-sticky-row:active,
            .craic-tree-view .repo-browser-row.repo-browser-sticky-row.selected {
                background-color: @window_bg_color;
                color: @view_fg_color;
            }
            .craic-tree-view .repo-browser-ignored-content {
                color: alpha(@view_fg_color, 0.55);
            }
            "#,
        );
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

pub struct TreeView<K, S> {
    pub root: gtk::Overlay,
    pub list: gtk::Box,
    pub scroller: gtk::ScrolledWindow,
    sticky_layer: gtk::Fixed,
    scrollbar: Option<TreeCanvasScrollbar>,
    rows: RefCell<Vec<TreeRow<K, S>>>,
    renderer: RefCell<Option<TreeRenderer<K, S>>>,
    row_reconciler: RefCell<crate::reconcile::gtk::BoxReconciler<K, TreeRenderState<K, S>>>,
    sticky_reconciler: RefCell<crate::reconcile::gtk::FixedReconciler<K, TreeRenderState<K, S>>>,
    disclosure: DisclosureAnimator<K>,
    sticky_signature: RefCell<Vec<K>>,
}

impl<K, S> TreeView<K, S>
where
    K: Clone + Eq + Hash + std::fmt::Debug + 'static,
    S: Clone + PartialEq + 'static,
{
    pub fn builder() -> TreeViewBuilder {
        TreeViewBuilder::new()
    }
}

pub struct TreeViewBuilder {
    vscrollbar_policy: gtk::PolicyType,
    autoscroll_context: &'static str,
    canvas_scrollbar: bool,
}

impl TreeViewBuilder {
    fn new() -> Self {
        Self {
            vscrollbar_policy: gtk::PolicyType::Automatic,
            autoscroll_context: "tree_view",
            canvas_scrollbar: false,
        }
    }

    pub fn vscrollbar_policy(mut self, policy: gtk::PolicyType) -> Self {
        self.vscrollbar_policy = policy;
        self
    }

    pub fn autoscroll_context(mut self, context: &'static str) -> Self {
        self.autoscroll_context = context;
        self
    }

    pub fn canvas_scrollbar(mut self, enabled: bool) -> Self {
        self.canvas_scrollbar = enabled;
        self
    }

    pub fn build<K, S>(self) -> Rc<TreeView<K, S>>
    where
        K: Clone + Eq + Hash + std::fmt::Debug + 'static,
        S: Clone + PartialEq + 'static,
    {
        install_css();

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .build();
        list.add_css_class("repo-browser-list");

        let viewport = gtk::Viewport::builder()
            .scroll_to_focus(false)
            .child(&list)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(self.vscrollbar_policy)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .vexpand(true)
            .child(&viewport)
            .build();

        let sticky_layer = gtk::Fixed::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Start)
            .hexpand(true)
            .visible(false)
            .build();
        let autoscroll_marker = gtk::DrawingArea::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();

        let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        root.add_css_class("craic-tree-view");
        root.set_child(Some(&scroller));
        root.add_overlay(&sticky_layer);
        root.add_overlay(&autoscroll_marker);
        canvas_scroll::install_scrolled_window_middle_autoscroll(
            &scroller,
            &autoscroll_marker,
            canvas_scroll::AutoscrollAxes::Vertical,
            self.autoscroll_context,
        );

        let scrollbar = self.canvas_scrollbar.then(|| {
            let area = gtk::DrawingArea::builder()
                .content_width(canvas_scrollbar::WIDTH.ceil() as i32)
                .width_request(canvas_scrollbar::WIDTH.ceil() as i32)
                .halign(gtk::Align::End)
                .valign(gtk::Align::Fill)
                .vexpand(true)
                .visible(false)
                .build();
            root.add_overlay(&area);
            TreeCanvasScrollbar {
                area,
                hover: Rc::new(Cell::new(false)),
                active: Rc::new(Cell::new(false)),
                hover_progress: Rc::new(Cell::new(0.0)),
                animating: Rc::new(Cell::new(false)),
                smooth_scroll: canvas_scrollbar::SmoothScroll::new(),
            }
        });

        let tree = Rc::new(TreeView {
            root,
            list,
            scroller,
            sticky_layer,
            scrollbar,
            rows: RefCell::new(Vec::new()),
            renderer: RefCell::new(None),
            row_reconciler: RefCell::new(crate::reconcile::gtk::BoxReconciler::new()),
            sticky_reconciler: RefCell::new(crate::reconcile::gtk::FixedReconciler::new()),
            disclosure: DisclosureAnimator::new(),
            sticky_signature: RefCell::new(Vec::new()),
        });
        tree.connect_sticky_scroll();
        tree.connect_canvas_scrollbar();
        tree
    }
}

struct TreeCanvasScrollbar {
    area: gtk::DrawingArea,
    hover: Rc<Cell<bool>>,
    active: Rc<Cell<bool>>,
    hover_progress: Rc<Cell<f64>>,
    animating: Rc<Cell<bool>>,
    smooth_scroll: canvas_scrollbar::SmoothScroll,
}

impl<K, S> TreeView<K, S>
where
    K: Clone + Eq + Hash + std::fmt::Debug + 'static,
    S: Clone + PartialEq + 'static,
{
    pub fn set_rows(
        self: &Rc<Self>,
        rows: Vec<TreeRow<K, S>>,
        renderer: TreeRenderer<K, S>,
    ) -> crate::reconcile::ReconcileStats {
        self.renderer.replace(Some(renderer.clone()));
        let width = self.list.allocated_width().max(1);
        let elements = rows
            .iter()
            .cloned()
            .map(|row| {
                Element::new(
                    row.key.clone(),
                    TreeRenderState {
                        row,
                        sticky: false,
                        bottom: false,
                        y: 0.0,
                        width,
                    },
                )
            })
            .collect::<Vec<_>>();
        let stats = self.row_reconciler.borrow_mut().reconcile(
            &self.list,
            elements,
            PartialEqRenderState,
            |index, key, state| (renderer.mount)(index, key, state),
            |index, widget, previous, next| (renderer.update)(index, widget, previous, next),
        );
        self.rows.replace(rows);
        self.update_sticky_rows();
        self.update_canvas_scrollbar();
        stats
    }

    pub fn clear(&self) {
        self.rows.borrow_mut().clear();
        self.row_reconciler.borrow_mut().reconcile(
            &self.list,
            std::iter::empty::<Element<K, TreeRenderState<K, S>>>(),
            PartialEqRenderState,
            |_, _, _| unreachable!("empty reconcile cannot mount tree rows"),
            |_, _, _, _| {},
        );
        self.clear_sticky_rows();
        self.update_canvas_scrollbar();
    }

    pub fn update_sticky_rows(&self) {
        let Some(renderer) = self.renderer.borrow().clone() else {
            self.clear_sticky_rows();
            return;
        };

        let scroll_y = self.scroller.vadjustment().value().max(0.0);
        let rows = self.rows.borrow().clone();
        let items = sticky_items(&rows, scroll_y);
        if items.is_empty() {
            self.clear_sticky_rows();
            return;
        }

        let overlay_height = items
            .last()
            .map(|item| item.visible_bottom - scroll_y)
            .unwrap_or(0.0)
            .max(1.0) as i32;
        let row_width = self.list.allocated_width().max(1);
        if !self.sticky_layer.is_visible() {
            self.sticky_layer.set_visible(true);
        }
        self.sticky_layer
            .set_size_request(row_width, overlay_height);

        let bottom_key = items.last().map(|item| item.row.key.clone());
        let signature = items
            .iter()
            .map(|item| item.row.key.clone())
            .collect::<Vec<_>>();
        if *self.sticky_signature.borrow() != signature {
            log::debug!(
                "tree sticky layout scroll_y={scroll_y:.1} overlay_height={overlay_height} rows={signature:?}"
            );
            self.sticky_signature.replace(signature);
        }

        let elements = items
            .into_iter()
            .rev()
            .map(|item| {
                let key = item.row.key.clone();
                Element::new(
                    key,
                    TreeRenderState {
                        bottom: bottom_key.as_ref() == Some(&item.row.key),
                        y: item.draw_y - scroll_y,
                        width: row_width,
                        sticky: true,
                        row: item.row,
                    },
                )
            })
            .collect::<Vec<_>>();

        self.sticky_reconciler.borrow_mut().reconcile(
            &self.sticky_layer,
            elements,
            PartialEqRenderState,
            |state| (0.0, state.y),
            |index, key, state| (renderer.mount)(index, key, state),
            |index, widget, previous, next| (renderer.update)(index, widget, previous, next),
        );
    }

    pub fn sticky_row_at_viewport_y(&self, y: f64) -> Option<TreeRow<K, S>> {
        let scroll_y = self.scroller.vadjustment().value().max(0.0);
        let content_y = y + scroll_y;
        sticky_items(&self.rows.borrow(), scroll_y)
            .into_iter()
            .find(|item| content_y >= item.draw_y && content_y < item.visible_bottom)
            .map(|item| item.row)
    }

    pub fn last_sticky_row(&self) -> Option<TreeRow<K, S>> {
        let scroll_y = self.scroller.vadjustment().value().max(0.0);
        sticky_items(&self.rows.borrow(), scroll_y)
            .last()
            .map(|item| item.row.clone())
    }

    pub fn row_at_content_y(&self, y: f64) -> Option<TreeRow<K, S>> {
        if y < 0.0 {
            return None;
        }

        let mut top = 0.0;
        for row in self.rows.borrow().iter() {
            let bottom = top + row.height;
            if y >= top && y < bottom {
                return Some(row.clone());
            }
            top = bottom;
        }
        None
    }

    pub fn last_row_before_content_y_matching<F>(
        &self,
        y: f64,
        mut matches: F,
    ) -> Option<TreeRow<K, S>>
    where
        F: FnMut(&TreeRow<K, S>) -> bool,
    {
        if y < 0.0 {
            return None;
        }

        let mut top = 0.0;
        let mut previous = None;
        for row in self.rows.borrow().iter() {
            if top > y {
                break;
            }
            if matches(row) {
                previous = Some(row.clone());
            }
            top += row.height;
        }
        previous
    }

    pub fn scroll_row_into_view(&self, key: &K) {
        let Some((row_top, row_height, top_inset)) = self.row_scroll_geometry(key) else {
            return;
        };

        let adjustment = self.scroller.vadjustment();
        let page_size = adjustment.page_size().max(1.0);
        let top_inset = top_inset.min((page_size - row_height).max(0.0));
        let view_top = adjustment.value() + top_inset;
        let view_bottom = adjustment.value() + page_size;
        let row_bottom = row_top + row_height;

        let target = if row_top < view_top {
            Some(row_top - top_inset)
        } else if row_bottom > view_bottom {
            let mut target = row_bottom - page_size;
            if row_top < target + top_inset {
                target = row_top - top_inset;
            }
            Some(target)
        } else {
            None
        };
        if let Some(target) = target {
            set_scroll_value(&adjustment, target);
        }
    }

    pub fn focus_row(&self, key: &K) -> bool {
        let Some(button) = self
            .row_widget(key)
            .and_then(|widget| widget.first_child())
            .and_then(|child| child.downcast::<gtk::Button>().ok())
        else {
            return false;
        };
        button.grab_focus()
    }

    pub fn focus_edit_row(&self, key: &K, placement: EditFocusPlacement) -> bool {
        let Some(entry) = self.row_widget(key).and_then(icon_row_entry) else {
            return false;
        };
        let focused = entry.grab_focus();
        match placement {
            EditFocusPlacement::Start => entry.set_position(0),
            EditFocusPlacement::SelectBeforeFirstDot => {
                entry.select_region(0, position_before_first_dot(&entry.text()));
            }
        }
        focused
    }

    pub fn has_row_focus(&self) -> bool {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if widget_tree_has_focus(&widget) {
                return true;
            }
            child = widget.next_sibling();
        }
        false
    }

    pub fn has_edit_focus(&self) -> bool {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if icon_row_entry(widget.clone()).is_some_and(|entry| entry.has_focus()) {
                return true;
            }
            child = widget.next_sibling();
        }
        false
    }

    pub fn connect_pointer_press<F>(self: &Rc<Self>, callback: F)
    where
        F: Fn(&gtk::GestureClick, f64, f64, f64, Option<TreeRow<K, S>>) + 'static,
    {
        let callback: Rc<PointerPressFn<K, S>> = Rc::new(callback);
        let click = gtk::GestureClick::builder().button(0).build();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed({
            let tree = self.clone();

            move |gesture, _, x, y| {
                let content_y = y + tree.scroller.vadjustment().value();
                callback(gesture, x, y, content_y, tree.row_at_content_y(content_y));
            }
        });
        self.scroller.add_controller(click);
    }

    pub fn scroll_row_below_sticky(&self, key: &K) {
        let Some((row_top, _, top_inset)) = self.row_scroll_geometry(key) else {
            return;
        };
        set_scroll_value(&self.scroller.vadjustment(), (row_top - top_inset).max(0.0));
    }

    pub fn row_is_below_sticky(&self, key: &K) -> bool {
        let Some((row_top, _, top_inset)) = self.row_scroll_geometry(key) else {
            return false;
        };
        let expected_scroll = (row_top - top_inset).max(0.0);
        (self.scroller.vadjustment().value() - expected_scroll).abs() < 0.5
    }

    fn row_scroll_geometry(&self, key: &K) -> Option<(f64, f64, f64)> {
        let rows = self.rows.borrow();
        let mut row_top = 0.0;
        for row in rows.iter() {
            if row.key == *key {
                return Some((row_top, row.height, row.depth as f64 * row.height));
            }
            row_top += row.height;
        }
        None
    }

    fn row_widget(&self, key: &K) -> Option<gtk::Widget> {
        let index = self.rows.borrow().iter().position(|row| row.key == *key)?;
        child_at_index(&self.list, index)
    }

    pub fn draw_disclosure(
        &self,
        key: &K,
        area: &gtk::DrawingArea,
        context: &gtk::cairo::Context,
        width: i32,
        height: i32,
    ) {
        self.disclosure.draw(key, area, context, width, height);
    }

    pub fn prepare_disclosure(&self, key: &K, expanded: bool) -> bool {
        self.disclosure.prepare(key, expanded)
    }

    pub fn animate_disclosure(self: &Rc<Self>, area: &gtk::DrawingArea, key: K) {
        let tree = self.clone();
        area.add_tick_callback(move |area, _| {
            let done = tree.disclosure.advance(&key);
            area.queue_draw();
            if done {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    pub fn disclosure_widget(self: &Rc<Self>, key: K, expanded: bool) -> gtk::DrawingArea {
        let handle = gtk::DrawingArea::builder()
            .content_width(ICON_SIZE)
            .content_height(ICON_SIZE)
            .width_request(ICON_SIZE)
            .height_request(ICON_SIZE)
            .valign(gtk::Align::Center)
            .build();
        handle.add_css_class("repo-browser-disclosure");

        let should_animate = self.prepare_disclosure(&key, expanded);
        handle.set_draw_func({
            let tree = self.clone();
            let key = key.clone();
            move |area, context, width, height| {
                tree.draw_disclosure(&key, area, context, width, height);
            }
        });
        if should_animate {
            self.animate_disclosure(&handle, key);
        }
        handle
    }

    fn connect_sticky_scroll(self: &Rc<Self>) {
        let sticky_scroll =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        sticky_scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        sticky_scroll.connect_scroll({
            let tree = self.clone();

            move |_, _, dy| {
                if dy.abs() <= f64::EPSILON {
                    return glib::Propagation::Proceed;
                }

                let adjustment = tree.scroller.vadjustment();
                let step = adjustment.step_increment().max(1.0);
                if let Some(scrollbar) = tree.scrollbar.as_ref() {
                    scrollbar.smooth_scroll.pause();
                }
                adjustment.set_value((adjustment.value() + dy * step).clamp(
                    adjustment.lower(),
                    (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
                ));
                tree.update_sticky_rows();
                glib::Propagation::Stop
            }
        });
        self.sticky_layer.add_controller(sticky_scroll);

        self.scroller.vadjustment().connect_value_changed({
            let tree = self.clone();
            move |_| {
                tree.update_sticky_rows();
                tree.list.queue_draw();
                tree.update_canvas_scrollbar();
            }
        });
    }

    fn connect_canvas_scrollbar(self: &Rc<Self>) {
        let Some(scrollbar) = &self.scrollbar else {
            return;
        };
        let real_adjustment = self.scroller.vadjustment();
        let scroll_drag = Rc::new(Cell::new(None::<canvas_scrollbar::Drag>));

        scrollbar.area.set_draw_func({
            let tree = self.clone();

            move |_, context, width, height| {
                tree.draw_canvas_scrollbar(context, width, height);
            }
        });

        real_adjustment.connect_upper_notify({
            let tree = self.clone();

            move |_| tree.update_canvas_scrollbar()
        });
        real_adjustment.connect_page_size_notify({
            let tree = self.clone();

            move |_| tree.update_canvas_scrollbar()
        });

        scrollbar.area.connect_resize({
            let tree = self.clone();

            move |_, _, _| tree.update_canvas_scrollbar()
        });

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let tree = self.clone();

            move |_, x, _| {
                if let Some(scrollbar) = tree.scrollbar.as_ref() {
                    canvas_scrollbar::set_hover(
                        &scrollbar.area,
                        &scrollbar.hover,
                        &scrollbar.active,
                        &scrollbar.hover_progress,
                        &scrollbar.animating,
                        tree.canvas_scrollbar_total_height()
                            .is_some_and(|total_height| {
                                canvas_scrollbar::point_in_lane(
                                    scrollbar.area.allocated_width(),
                                    scrollbar.area.allocated_height(),
                                    total_height,
                                    x,
                                )
                            }),
                    );
                }
            }
        });
        motion.connect_leave({
            let tree = self.clone();

            move |_| {
                if let Some(scrollbar) = tree.scrollbar.as_ref() {
                    canvas_scrollbar::set_hover(
                        &scrollbar.area,
                        &scrollbar.hover,
                        &scrollbar.active,
                        &scrollbar.hover_progress,
                        &scrollbar.animating,
                        false,
                    );
                }
            }
        });
        scrollbar.area.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll({
            let tree = self.clone();

            move |controller, _, dy| {
                if dy.abs() <= f64::EPSILON {
                    return glib::Propagation::Proceed;
                }
                let Some(scrollbar) = tree.scrollbar.as_ref() else {
                    return glib::Propagation::Proceed;
                };
                let height = scrollbar.area.allocated_height();
                let page_size = tree.external_scroll_page_size_for_height(height);
                let adjustment = tree.scroller.vadjustment();
                let lower = adjustment.lower();
                let upper = external_scroll_max(&adjustment, page_size);
                let current = tree.external_scroll_value_for_real(&adjustment, page_size);
                if canvas_scrollbar::is_mouse_scroll(controller) {
                    let delta = canvas_scrollbar::mouse_wheel_delta(page_size, dy);
                    let tree_for_scroll = tree.clone();
                    scrollbar.smooth_scroll.scroll_relative(
                        &scrollbar.area,
                        current,
                        delta,
                        lower,
                        upper,
                        move |value| tree_for_scroll.set_external_scroll_value(value),
                    );
                } else {
                    scrollbar.smooth_scroll.pause();
                    let step = adjustment.step_increment().max(1.0);
                    tree.set_external_scroll_value(current + dy * step);
                }
                glib::Propagation::Stop
            }
        });
        scrollbar.area.add_controller(scroll);

        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_pressed({
            let tree = self.clone();

            move |_, _, x, y| {
                tree.scrollbar_press(x, y);
            }
        });
        scrollbar.area.add_controller(click);

        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        drag.connect_drag_begin({
            let tree = self.clone();
            let scroll_drag = scroll_drag.clone();

            move |_, x, y| {
                if let Some(scroll_y) = tree.scrollbar_press(x, y) {
                    if let Some(scrollbar) = tree.scrollbar.as_ref() {
                        canvas_scrollbar::set_active(
                            &scrollbar.area,
                            &scrollbar.hover,
                            &scrollbar.active,
                            &scrollbar.hover_progress,
                            &scrollbar.animating,
                            true,
                        );
                    }
                    scroll_drag.set(Some(canvas_scrollbar::Drag::new(scroll_y)));
                } else {
                    if let Some(scrollbar) = tree.scrollbar.as_ref() {
                        canvas_scrollbar::set_active(
                            &scrollbar.area,
                            &scrollbar.hover,
                            &scrollbar.active,
                            &scrollbar.hover_progress,
                            &scrollbar.animating,
                            false,
                        );
                    }
                    scroll_drag.set(None);
                }
            }
        });
        drag.connect_drag_update({
            let tree = self.clone();
            let scroll_drag = scroll_drag.clone();

            move |_, _, offset_y| {
                let Some(drag) = scroll_drag.get() else {
                    return;
                };
                let Some(scrollbar) = tree.scrollbar.as_ref() else {
                    return;
                };
                let Some(total_height) = tree.canvas_scrollbar_total_height() else {
                    return;
                };
                let width = scrollbar.area.allocated_width();
                let height = scrollbar.area.allocated_height();
                let scroll_y = tree.external_scroll_value();
                let Some((_, _, _, thumb_height)) =
                    canvas_scrollbar::thumb_rect(width, height, total_height, scroll_y)
                else {
                    return;
                };
                let viewport_height = height.max(1) as f64;
                let max_scroll = canvas_scrollbar::max_scroll(total_height, viewport_height);
                tree.set_external_scroll_value(drag.scroll_for_delta(
                    offset_y,
                    viewport_height,
                    thumb_height,
                    max_scroll,
                ));
            }
        });
        drag.connect_drag_end({
            let tree = self.clone();
            let scroll_drag = scroll_drag.clone();

            move |_, _, _| {
                scroll_drag.set(None);
                if let Some(scrollbar) = tree.scrollbar.as_ref() {
                    canvas_scrollbar::set_active(
                        &scrollbar.area,
                        &scrollbar.hover,
                        &scrollbar.active,
                        &scrollbar.hover_progress,
                        &scrollbar.animating,
                        false,
                    );
                }
            }
        });
        scrollbar.area.add_controller(drag);
    }

    fn update_canvas_scrollbar(&self) {
        let Some(scrollbar) = &self.scrollbar else {
            return;
        };
        scrollbar
            .area
            .set_visible(self.canvas_scrollbar_total_height().is_some());
        scrollbar.area.queue_draw();
    }

    fn draw_canvas_scrollbar(&self, context: &gtk::cairo::Context, width: i32, height: i32) {
        let Some(scrollbar) = &self.scrollbar else {
            return;
        };
        let Some(total_height) = self.canvas_scrollbar_total_height_for_height(height) else {
            return;
        };
        let scroll_y = self.external_scroll_value_for_height(height);
        let hover = scrollbar.hover_progress.get().clamp(0.0, 1.0);
        let active = scrollbar.active.get();
        let theme = canvas_scrollbar::Theme::for_widget(&scrollbar.area);
        canvas_scrollbar::draw_track(context, width, height, total_height, hover, theme);
        canvas_scrollbar::draw_thumb(
            context,
            width,
            height,
            total_height,
            scroll_y,
            hover,
            active,
            theme,
        );
    }

    fn scrollbar_press(self: &Rc<Self>, x: f64, y: f64) -> Option<f64> {
        let scrollbar = self.scrollbar.as_ref()?;
        let total_height = self.canvas_scrollbar_total_height()?;
        let width = scrollbar.area.allocated_width();
        let height = scrollbar.area.allocated_height();
        let scroll_y = self.external_scroll_value();
        let next_scroll_y =
            canvas_scrollbar::scroll_for_lane_press(width, height, total_height, scroll_y, x, y)?;
        scrollbar.smooth_scroll.pause();
        self.set_external_scroll_value(next_scroll_y);
        Some(next_scroll_y)
    }

    fn canvas_scrollbar_total_height(&self) -> Option<f64> {
        self.scrollbar.as_ref().and_then(|scrollbar| {
            self.canvas_scrollbar_total_height_for_height(scrollbar.area.allocated_height())
        })
    }

    fn canvas_scrollbar_total_height_for_height(&self, height: i32) -> Option<f64> {
        let page_size = self.external_scroll_page_size_for_height(height);
        let real_adjustment = self.scroller.vadjustment();
        let external_max = external_scroll_max(&real_adjustment, page_size);
        (external_max > real_adjustment.lower()).then_some(external_max + page_size)
    }

    fn external_scroll_value(&self) -> f64 {
        self.scrollbar
            .as_ref()
            .map(|scrollbar| {
                self.external_scroll_value_for_height(scrollbar.area.allocated_height())
            })
            .unwrap_or_else(|| self.scroller.vadjustment().value())
    }

    fn external_scroll_value_for_height(&self, height: i32) -> f64 {
        let real_adjustment = self.scroller.vadjustment();
        let page_size = self.external_scroll_page_size_for_height(height);
        self.external_scroll_value_for_real(&real_adjustment, page_size)
    }

    fn set_external_scroll_value(self: &Rc<Self>, external_value: f64) {
        let real_adjustment = self.scroller.vadjustment();
        let page_size = self
            .scrollbar
            .as_ref()
            .map(|scrollbar| {
                self.external_scroll_page_size_for_height(scrollbar.area.allocated_height())
            })
            .unwrap_or_else(|| self.external_scroll_page_size(&real_adjustment));
        let real_value = self.real_scroll_value_for_external(external_value, page_size);
        set_scroll_value(&real_adjustment, real_value);
        self.update_sticky_rows();
        self.list.queue_draw();
        self.update_canvas_scrollbar();
    }

    fn external_scroll_page_size_for_height(&self, height: i32) -> f64 {
        if height > 0 {
            height as f64
        } else {
            self.external_scroll_page_size(&self.scroller.vadjustment())
        }
    }

    fn external_scroll_page_size(&self, real_adjustment: &gtk::Adjustment) -> f64 {
        let body_height = self.root.allocated_height();
        if body_height > 0 {
            body_height as f64
        } else {
            real_adjustment.page_size()
        }
        .max(1.0)
    }

    fn external_scroll_value_for_real(
        &self,
        real_adjustment: &gtk::Adjustment,
        external_page_size: f64,
    ) -> f64 {
        let lower = real_adjustment.lower();
        let real_max = real_scroll_max(real_adjustment);
        let external_max = external_scroll_max(real_adjustment, external_page_size);
        map_scroll_value(real_adjustment.value(), lower, real_max, external_max)
    }

    fn real_scroll_value_for_external(&self, external_value: f64, external_page_size: f64) -> f64 {
        let real_adjustment = self.scroller.vadjustment();
        let lower = real_adjustment.lower();
        let external_max = external_scroll_max(&real_adjustment, external_page_size);
        let real_max = real_scroll_max(&real_adjustment);
        map_scroll_value(external_value, lower, external_max, real_max)
    }

    fn clear_sticky_rows(&self) {
        self.sticky_reconciler.borrow_mut().reconcile(
            &self.sticky_layer,
            std::iter::empty::<Element<K, TreeRenderState<K, S>>>(),
            PartialEqRenderState,
            |state| (0.0, state.y),
            |_, _, _| unreachable!("empty reconcile cannot mount sticky rows"),
            |_, _, _, _| {},
        );
        self.sticky_signature.borrow_mut().clear();
        if self.sticky_layer.is_visible() {
            self.sticky_layer.set_visible(false);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditFocusPlacement {
    Start,
    SelectBeforeFirstDot,
}

fn position_before_first_dot(text: &str) -> i32 {
    text.chars()
        .position(|ch| ch == '.')
        .unwrap_or_else(|| text.chars().count()) as i32
}

fn set_scroll_value(adjustment: &gtk::Adjustment, value: f64) {
    adjustment.set_value(
        value
            .min(adjustment.upper() - adjustment.page_size())
            .max(adjustment.lower()),
    );
}

fn real_scroll_max(adjustment: &gtk::Adjustment) -> f64 {
    (adjustment.upper() - adjustment.page_size()).max(adjustment.lower())
}

fn external_scroll_max(real_adjustment: &gtk::Adjustment, external_page_size: f64) -> f64 {
    (real_adjustment.upper() - external_page_size).max(real_adjustment.lower())
}

fn map_scroll_value(value: f64, lower: f64, from_max: f64, to_max: f64) -> f64 {
    if from_max <= lower || to_max <= lower {
        return lower;
    }

    let progress = ((value - lower) / (from_max - lower)).clamp(0.0, 1.0);
    lower + progress * (to_max - lower)
}

fn child_at_index(parent: &gtk::Box, index: usize) -> Option<gtk::Widget> {
    let mut child = parent.first_child();
    for _ in 0..index {
        child = child?.next_sibling();
    }
    child
}

fn widget_tree_has_focus(widget: &gtk::Widget) -> bool {
    if widget.has_focus() {
        return true;
    }

    let mut child = widget.first_child();
    while let Some(child_widget) = child {
        if widget_tree_has_focus(&child_widget) {
            return true;
        }
        child = child_widget.next_sibling();
    }
    false
}
