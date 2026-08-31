use craic_render_skia::{
    DiffDocument, DiffFoldRange, DiffLayoutCache, DiffLayoutRequest, DiffLayoutSignature,
    DiffMarkerKind, DiffPaintRequest, DiffRow, DiffRowKind, DiffSearchMatch, DiffSide,
    DiffSyntaxSpan, DiffTextPoint, DiffTextSelection, build_diff_layout, build_initial_diff_folds,
    diff_row_index_at_y, display_diff_rows, find_diff_search_matches, paint_diff,
    select_all_diff_text, selected_diff_text,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityTextAreaRole, NSAutoresizingMaskOptions, NSBorderType,
    NSCursor, NSEvent, NSEventModifierFlags, NSEventTrackingRunLoopMode, NSMenu, NSMenuItem,
    NSPasteboard, NSPasteboardTypeString, NSScrollElasticity, NSScrollView, NSSearchField,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSViewBoundsDidChangeNotification,
    NSWindowOcclusionState,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect,
    NSRunLoop, NSRunLoopCommonModes, NSSize, NSString,
};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLDrawable,
    MTLPixelFormat,
};
use objc2_quartz_core::{
    CAMetalDisplayLink, CAMetalDisplayLinkDelegate, CAMetalDisplayLinkUpdate, CAMetalDrawable,
    CAMetalLayer,
};
use skia_safe::gpu::{self, DirectContext, SurfaceOrigin, backend_render_targets, mtl};
use skia_safe::{Color4f, ColorType, Paint, Rect};
use std::cell::{Cell, RefCell};

const BASE_FONT_SIZE: f64 = 13.0;
const BASE_LINE_HEIGHT: f64 = 22.0;
const BASE_CHAR_WIDTH: f64 = 7.83;
const BASELINE_OFFSET_RATIO: f64 = 16.5 / BASE_FONT_SIZE;
const GUTTER_WIDTH: f64 = 58.0;
const CELL_PADDING: f64 = 10.0;
const SCROLLBAR_WIDTH: f64 = 24.0;
const SCROLLBAR_IDLE_LANE_WIDTH: f64 = 11.0;
const SCROLLBAR_IDLE_MARGIN: f64 = 4.0;
const SCROLLBAR_HOVER_MARGIN: f64 = 8.0;
const SCROLLBAR_VERTICAL_MARGIN: f64 = 9.0;
const SCROLLBAR_MIN_THUMB: f64 = 40.0;
const SCROLLBAR_ANIMATION_DURATION_SECONDS: f64 = 0.2;
const DIFF_ACCESSIBILITY_MAX_BYTES: usize = 256 * 1024;

struct MetalState {
    _device: Retained<ProtocolObject<dyn MTLDevice>>,
    layer: Retained<CAMetalLayer>,
    command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    skia: DirectContext,
}

impl Drop for MetalState {
    fn drop(&mut self) {
        log::info!("releasing native Skia Metal diff resources");
        self.skia.release_resources_and_abandon();
    }
}

#[derive(Default)]
struct DiffState {
    path: String,
    fingerprint: u64,
    full_rows: Vec<DiffRow>,
    folds: Vec<DiffFoldRange>,
    rows: Vec<DiffRow>,
    layout: Option<DiffLayoutCache>,
    generation: u64,
    layout_width: i32,
    scroll_y: f64,
    elastic_scrolling: bool,
    selection: Option<DiffTextSelection>,
    selection_anchor: Option<DiffTextPoint>,
    active_side: Option<DiffSide>,
    search_query: String,
    search_matches: Vec<DiffSearchMatch>,
    active_search_match: Option<usize>,
    syntax: Vec<DiffSyntaxSpan>,
    font_size: f64,
    line_height: f64,
    char_width: f64,
}

fn scrollbar_thumb_geometry(
    viewport_height: f64,
    total_height: f64,
    scroll_y: f64,
) -> Option<(f64, f64)> {
    if total_height <= viewport_height + 0.5 {
        return None;
    }
    let track_height = (viewport_height - SCROLLBAR_VERTICAL_MARGIN * 2.0).max(1.0);
    let thumb_height = (track_height * viewport_height / total_height)
        .max(SCROLLBAR_MIN_THUMB)
        .min(track_height);
    let maximum = (total_height - viewport_height).max(1.0);
    let travel = (track_height - thumb_height).max(0.0);
    let y = SCROLLBAR_VERTICAL_MARGIN + scroll_y.clamp(0.0, maximum) / maximum * travel;
    Some((y, thumb_height))
}

fn scrollbar_scroll_for_press(
    viewport_height: f64,
    total_height: f64,
    scroll_y: f64,
    y: f64,
) -> Option<f64> {
    let (thumb_y, thumb_height) =
        scrollbar_thumb_geometry(viewport_height, total_height, scroll_y)?;
    if y >= thumb_y && y <= thumb_y + thumb_height {
        return Some(scroll_y.clamp(0.0, (total_height - viewport_height).max(0.0)));
    }
    let track_height = (viewport_height - SCROLLBAR_VERTICAL_MARGIN * 2.0).max(1.0);
    let travel = (track_height - thumb_height).max(0.0);
    let maximum = (total_height - viewport_height).max(0.0);
    if travel <= f64::EPSILON || maximum <= f64::EPSILON {
        return Some(0.0);
    }
    let thumb_y = (y - thumb_height / 2.0).clamp(
        SCROLLBAR_VERTICAL_MARGIN,
        SCROLLBAR_VERTICAL_MARGIN + travel,
    );
    Some((thumb_y - SCROLLBAR_VERTICAL_MARGIN) / travel * maximum)
}

fn draw_skia_scrollbar(
    canvas: &skia_safe::Canvas,
    width: f64,
    height: f64,
    layout: &DiffLayoutCache,
    scroll_y: f64,
    hover: f64,
    active: bool,
) {
    let Some((thumb_y, thumb_height)) =
        scrollbar_thumb_geometry(height, layout.content_height, scroll_y)
    else {
        return;
    };
    let hover = hover.clamp(0.0, 1.0);
    let lane_width =
        SCROLLBAR_IDLE_LANE_WIDTH + (SCROLLBAR_WIDTH - SCROLLBAR_IDLE_LANE_WIDTH) * hover;
    let margin = SCROLLBAR_IDLE_MARGIN + (SCROLLBAR_HOVER_MARGIN - SCROLLBAR_IDLE_MARGIN) * hover;
    let handle_x = width - lane_width + margin;
    let handle_width = (lane_width - margin * 2.0).max(1.0);
    let track_height = (height - SCROLLBAR_VERTICAL_MARGIN * 2.0).max(1.0);
    let rounded = |x: f64, y: f64, w: f64, h: f64, color: Color4f| {
        let mut paint = Paint::new(color, None);
        paint.set_anti_alias(true);
        canvas.draw_round_rect(
            Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
            (w / 2.0) as f32,
            (w / 2.0) as f32,
            &paint,
        );
    };

    if hover > f64::EPSILON {
        rounded(
            handle_x,
            SCROLLBAR_VERTICAL_MARGIN,
            handle_width,
            track_height,
            Color4f::new(1.0, 1.0, 1.0, (0.10 * hover) as f32),
        );
    }

    canvas.save();
    canvas.clip_rect(
        Rect::from_xywh(
            handle_x as f32,
            SCROLLBAR_VERTICAL_MARGIN as f32,
            handle_width as f32,
            track_height as f32,
        ),
        None,
        false,
    );
    for marker in &layout.markers {
        let Some(row) = layout.rows.get(marker.row) else {
            continue;
        };
        let marker_y =
            SCROLLBAR_VERTICAL_MARGIN + row.y / layout.content_height.max(1.0) * track_height;
        let marker_height = (row.height / layout.content_height.max(1.0) * track_height)
            .max(2.0)
            .min(SCROLLBAR_VERTICAL_MARGIN + track_height - marker_y);
        let alpha = (0.58 + (0.82 - 0.58) * hover) as f32;
        let marker_rect = |x: f64, marker_width: f64, color: Color4f| {
            let mut paint = Paint::new(color, None);
            paint.set_anti_alias(false);
            canvas.draw_rect(
                Rect::from_xywh(
                    x as f32,
                    marker_y as f32,
                    marker_width as f32,
                    marker_height as f32,
                ),
                &paint,
            );
        };
        match marker.kind {
            DiffMarkerKind::Added => marker_rect(
                handle_x,
                handle_width,
                Color4f::new(0.10, 0.52, 0.24, alpha),
            ),
            DiffMarkerKind::Deleted => marker_rect(
                handle_x,
                handle_width,
                Color4f::new(0.62, 0.12, 0.15, alpha),
            ),
            DiffMarkerKind::Mixed => {
                marker_rect(
                    handle_x,
                    handle_width / 2.0,
                    Color4f::new(0.62, 0.12, 0.15, alpha),
                );
                marker_rect(
                    handle_x + handle_width / 2.0,
                    handle_width / 2.0,
                    Color4f::new(0.10, 0.52, 0.24, alpha),
                );
            }
        }
    }
    canvas.restore();

    let outline_alpha = if active { 0.60 } else { 0.35 + 0.25 * hover };
    rounded(
        handle_x - 1.0,
        thumb_y - 1.0,
        handle_width + 2.0,
        thumb_height + 2.0,
        Color4f::new(0.0, 0.0, 0.0, (0.95 * outline_alpha) as f32),
    );
    let thumb_alpha = if active { 0.60 } else { 0.20 + 0.20 * hover };
    rounded(
        handle_x,
        thumb_y,
        handle_width,
        thumb_height,
        Color4f::new(1.0, 1.0, 1.0, thumb_alpha as f32),
    );
}

impl DiffState {
    fn set_document(
        &mut self,
        path: &str,
        fingerprint: u64,
        document: DiffDocument,
        syntax: Vec<DiffSyntaxSpan>,
    ) {
        let preserve = self.path == path;
        let previous_folds = if preserve {
            self.folds.clone()
        } else {
            Vec::new()
        };
        self.path = path.to_string();
        self.fingerprint = fingerprint;
        self.full_rows = document.rows;
        self.folds = build_initial_diff_folds(&self.full_rows, &previous_folds);
        self.rows = display_diff_rows(&self.full_rows, &self.folds);
        self.syntax = syntax;
        self.generation = self.generation.wrapping_add(1).max(1);
        self.layout = None;
        self.layout_width = 0;
        if !preserve {
            self.scroll_y = 0.0;
            self.elastic_scrolling = false;
            self.selection = None;
            self.selection_anchor = None;
        }
        self.rebuild_search_matches();
    }

    fn clear(&mut self) {
        let font_size = self.font_size;
        let line_height = self.line_height;
        let char_width = self.char_width;
        *self = Self {
            generation: self.generation.wrapping_add(1).max(1),
            font_size,
            line_height,
            char_width,
            ..Self::default()
        };
    }

    fn ensure_layout(&mut self, width: f64, height: f64) {
        let content_width = (width - SCROLLBAR_WIDTH).max(1.0);
        let layout_width = content_width.round() as i32;
        if self.layout.is_some() && self.layout_width == layout_width {
            if !self.elastic_scrolling {
                self.clamp_scroll(height);
            }
            return;
        }
        let half_width = content_width / 2.0;
        let text_width = (half_width - GUTTER_WIDTH - CELL_PADDING * 2.0).max(40.0);
        let signature = DiffLayoutSignature::new(
            self.generation,
            layout_width,
            GUTTER_WIDTH,
            self.line_height,
            text_width,
            self.char_width,
            self.rows.len(),
        );
        self.layout = Some(build_diff_layout(DiffLayoutRequest {
            signature,
            rows: self.rows.clone(),
            text_width,
            line_height: self.line_height,
            char_width: self.char_width,
        }));
        self.layout_width = layout_width;
        if !self.elastic_scrolling {
            self.clamp_scroll(height);
        }
    }

    fn maximum_scroll(&self, viewport_height: f64) -> f64 {
        self.layout
            .as_ref()
            .map(|layout| (layout.content_height - viewport_height).max(0.0))
            .unwrap_or(0.0)
    }

    fn clamp_scroll(&mut self, viewport_height: f64) {
        let maximum = self.maximum_scroll(viewport_height);
        self.scroll_y = self.scroll_y.clamp(0.0, maximum);
    }

    fn toggle_fold(&mut self, fold_index: usize, width: f64, height: f64) -> bool {
        let Some(fold) = self.folds.get_mut(fold_index) else {
            return false;
        };
        fold.expanded = !fold.expanded;
        self.rows = display_diff_rows(&self.full_rows, &self.folds);
        self.generation = self.generation.wrapping_add(1).max(1);
        self.layout = None;
        self.rebuild_search_matches();
        self.ensure_layout(width, height);
        true
    }

    fn rebuild_search_matches(&mut self) {
        self.active_search_match = None;
        let query = self.search_query.trim();
        if query.is_empty() {
            self.search_matches.clear();
            return;
        }
        self.search_matches = find_diff_search_matches(&self.rows, query);
        if !self.search_matches.is_empty() {
            self.active_search_match = Some(0);
        }
    }

    fn select_search_match(&mut self, delta: isize, viewport_height: f64) {
        if self.search_matches.is_empty() {
            self.active_search_match = None;
            return;
        }
        let len = self.search_matches.len() as isize;
        let current = self.active_search_match.unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        self.active_search_match = Some(next);
        let search_match = self.search_matches[next];
        self.selection = Some(DiffTextSelection {
            anchor: DiffTextPoint {
                side: search_match.side,
                row: search_match.row,
                byte: search_match.start,
            },
            focus: DiffTextPoint {
                side: search_match.side,
                row: search_match.row,
                byte: search_match.end,
            },
        });
        if let (Some(layout), Some(row)) = (
            self.layout.as_ref(),
            self.layout
                .as_ref()
                .and_then(|layout| layout.rows.get(search_match.row)),
        ) {
            if row.y < self.scroll_y {
                self.scroll_y = row.y;
            } else if row.y + row.height > self.scroll_y + viewport_height {
                self.scroll_y = row.y + row.height - viewport_height;
            }
            let maximum = (layout.content_height - viewport_height).max(0.0);
            self.scroll_y = self.scroll_y.clamp(0.0, maximum);
        }
    }
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct NativeScrollDocument;

    unsafe impl NSObjectProtocol for NativeScrollDocument {}

    impl NativeScrollDocument {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl NativeScrollDocument {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated frame initializer is valid for this private document view.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

pub(crate) struct DiffMetalViewIvars {
    metal: RefCell<Option<MetalState>>,
    state: RefCell<DiffState>,
    dragging_selection: Cell<bool>,
    search_field: RefCell<Option<Retained<NSSearchField>>>,
    search_panel: RefCell<Option<Retained<NSView>>>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
    scrollbar_hovered: Cell<bool>,
    scrollbar_active: Cell<bool>,
    scrollbar_hover_progress: Cell<f64>,
    scrollbar_drag_start_y: Cell<f64>,
    scrollbar_drag_start_scroll: Cell<f64>,
    display_link: RefCell<Option<Retained<CAMetalDisplayLink>>>,
    window_occluded: Cell<bool>,
    last_frame_timestamp: Cell<f64>,
    native_scroll: RefCell<Option<Retained<NSScrollView>>>,
    native_scroll_document: RefCell<Option<Retained<NativeScrollDocument>>>,
}

define_class!(
    // SAFETY: All drawing and input methods are confined to AppKit's main thread.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = DiffMetalViewIvars]
    pub(crate) struct DiffMetalView;

    unsafe impl NSObjectProtocol for DiffMetalView {}
    unsafe impl CAMetalDisplayLinkDelegate for DiffMetalView {
        #[unsafe(method(metalDisplayLink:needsUpdate:))]
        fn metal_display_link_needs_update(
            &self,
            display_link: &CAMetalDisplayLink,
            update: &CAMetalDisplayLinkUpdate,
        ) {
            if !self.can_render() {
                display_link.setPaused(true);
                return;
            }
            self.advance_animation(display_link, update);
        }
    }

    impl DiffMetalView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            self.render_frame();
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            if let Some(window) = self.window() {
                self.ivars().window_occluded.set(
                    !window
                        .occlusionState()
                        .contains(NSWindowOcclusionState::Visible),
                );
                self.initialize_display_link();
                self.render_frame();
            } else if let Some(display_link) = self.ivars().display_link.borrow_mut().take() {
                display_link.invalidate();
                self.ivars().last_frame_timestamp.set(0.0);
                log::debug!("native diff display link invalidated after leaving its window");
            }
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            self.render_frame();
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: Dispatching to NSView's implementation preserves AppKit layout bookkeeping.
            unsafe {
                let _: () = msg_send![super(self), setFrameSize: size];
            }
            self.update_native_scroll_geometry();
            self.render_frame();
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let bounds = self.bounds();
            let content_width = (bounds.size.width - SCROLLBAR_WIDTH).max(0.0);
            self.addCursorRect_cursor(
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(content_width, bounds.size.height)),
                &NSCursor::IBeamCursor(),
            );
            self.addCursorRect_cursor(
                NSRect::new(
                    NSPoint::new(content_width, 0.0),
                    NSSize::new(bounds.size.width - content_width, bounds.size.height),
                ),
                &NSCursor::arrowCursor(),
            );
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            if self.point_in_scrollbar_lane(point) {
                None
            } else {
                if let Some(window) = self.window() {
                    window.makeFirstResponder(Some(self));
                }
                let menu = NSMenu::new(self.mtm());
                let copy = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(self.mtm()),
                        &NSString::from_str("Copy"),
                        Some(objc2::sel!(copy:)),
                        &NSString::from_str("c"),
                    )
                };
                let select_all = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(self.mtm()),
                        &NSString::from_str("Select All"),
                        Some(objc2::sel!(selectAll:)),
                        &NSString::from_str("a"),
                    )
                };
                unsafe {
                    copy.setTarget(Some(self));
                    select_all.setTarget(Some(self));
                }
                copy.setEnabled(self.ivars().state.borrow().selection.is_some_and(|selection| {
                    selection.anchor != selection.focus
                }));
                menu.addItem(&copy);
                menu.addItem(&select_all);
                Some(menu)
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.update_native_scroll_geometry();
            if let Some(scroll) = self.ivars().native_scroll.borrow().as_ref() {
                scroll.scrollWheel(event);
            }
        }

        #[unsafe(method(nativeScrollBoundsChanged:))]
        fn native_scroll_bounds_changed(&self, _notification: &NSNotification) {
            self.apply_native_scroll_position();
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            self.set_scrollbar_hovered(self.point_in_scrollbar_lane(point));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.set_scrollbar_hovered(false);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self));
            }
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let bounds = self.bounds();
            let mut state = self.ivars().state.borrow_mut();
            state.ensure_layout(bounds.size.width, bounds.size.height);
            state.elastic_scrolling = false;
            if self.point_in_scrollbar_lane(point) {
                if let Some(scroll_y) = scrollbar_scroll_for_press(
                    bounds.size.height,
                    state.layout.as_ref().map_or(0.0, |layout| layout.content_height),
                    state.scroll_y,
                    point.y,
                ) {
                    state.scroll_y = scroll_y;
                    self.ivars().scrollbar_drag_start_y.set(point.y);
                    self.ivars().scrollbar_drag_start_scroll.set(scroll_y);
                    self.ivars().scrollbar_active.set(true);
                    self.ivars().dragging_selection.set(false);
                    drop(state);
                    self.sync_native_scroll_to_state();
                    self.start_display_link();
                    self.render_frame();
                    return;
                }
            }
            if let Some(row_index) = state.row_at_point(point) {
                if let Some(row) = state.rows.get(row_index) {
                    if row.left_kind == DiffRowKind::Fold || row.right_kind == DiffRowKind::Fold {
                        let fold_index = row.left_number.or(row.right_number);
                        if let Some(fold_index) = fold_index {
                            if state.toggle_fold(fold_index, bounds.size.width, bounds.size.height) {
                                log::debug!("native diff fold toggled index={fold_index}");
                            }
                        }
                        drop(state);
                        self.update_native_scroll_geometry();
                        self.sync_native_scroll_to_state();
                        self.update_accessibility_selection();
                        self.render_frame();
                        return;
                    }
                }
            }
            if let Some(text_point) =
                state.text_point_at(point, (bounds.size.width - SCROLLBAR_WIDTH).max(1.0))
            {
                state.active_side = Some(text_point.side);
                state.selection_anchor = Some(text_point);
                state.selection = Some(DiffTextSelection {
                    anchor: text_point,
                    focus: text_point,
                });
                self.ivars().dragging_selection.set(true);
            } else {
                state.selection = None;
                state.selection_anchor = None;
                self.ivars().dragging_selection.set(false);
            }
            drop(state);
            self.update_accessibility_selection();
            self.render_frame();
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if self.ivars().scrollbar_active.get() {
                let point = self.convertPoint_fromView(event.locationInWindow(), None);
                let bounds = self.bounds();
                let mut state = self.ivars().state.borrow_mut();
                state.ensure_layout(bounds.size.width, bounds.size.height);
                let total_height = state.layout.as_ref().map_or(0.0, |layout| layout.content_height);
                if let Some((_, thumb_height)) = scrollbar_thumb_geometry(
                    bounds.size.height,
                    total_height,
                    state.scroll_y,
                ) {
                    let track_height =
                        (bounds.size.height - SCROLLBAR_VERTICAL_MARGIN * 2.0).max(1.0);
                    let travel = (track_height - thumb_height).max(1.0);
                    let maximum = (total_height - bounds.size.height).max(0.0);
                    state.scroll_y = (self.ivars().scrollbar_drag_start_scroll.get()
                        + (point.y - self.ivars().scrollbar_drag_start_y.get()) / travel * maximum)
                        .clamp(0.0, maximum);
                }
                drop(state);
                self.sync_native_scroll_to_state();
                self.render_frame();
                return;
            }
            if !self.ivars().dragging_selection.get() {
                return;
            }
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let bounds = self.bounds();
            let mut state = self.ivars().state.borrow_mut();
            state.ensure_layout(bounds.size.width, bounds.size.height);
            let Some(anchor) = state.selection_anchor else { return };
            if point.y < 0.0 {
                state.scroll_y = (state.scroll_y - state.line_height).max(0.0);
            } else if point.y > bounds.size.height {
                state.scroll_y += state.line_height;
                state.clamp_scroll(bounds.size.height);
            }
            let hit_point = NSPoint::new(
                point.x,
                point.y.clamp(0.0, (bounds.size.height - 0.5).max(0.0)),
            );
            if let Some(focus) =
                state.selection_focus_at(
                    hit_point,
                    (bounds.size.width - SCROLLBAR_WIDTH).max(1.0),
                    anchor.side,
                )
            {
                state.selection = Some(DiffTextSelection { anchor, focus });
            }
            drop(state);
            self.sync_native_scroll_to_state();
            self.update_accessibility_selection();
            self.render_frame();
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().dragging_selection.set(false);
            if self.ivars().scrollbar_active.replace(false) {
                self.start_display_link();
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let characters = event
                .charactersIgnoringModifiers()
                .map(|characters| characters.to_string())
                .unwrap_or_default();
            let modifiers = event.modifierFlags();
            if modifiers.contains(NSEventModifierFlags::Command) {
                match characters.as_str() {
                    "c" => {
                        self.copy_selection();
                        return;
                    }
                    "a" => {
                        self.select_all();
                        return;
                    }
                    "f" => {
                        self.focus_search();
                        return;
                    }
                    "g" => {
                        if modifiers.contains(NSEventModifierFlags::Shift) {
                            self.search_previous();
                        } else {
                            self.search_next();
                        }
                        return;
                    }
                    _ => {}
                }
            }
            let bounds = self.bounds();
            let mut state = self.ivars().state.borrow_mut();
            state.ensure_layout(bounds.size.width, bounds.size.height);
            match event.keyCode() {
                123 => state.scroll_y = (state.scroll_y - state.char_width * 4.0).max(0.0),
                124 => state.scroll_y += state.char_width * 4.0,
                125 => state.scroll_y += state.line_height,
                126 => state.scroll_y = (state.scroll_y - state.line_height).max(0.0),
                116 => state.scroll_y = (state.scroll_y - bounds.size.height).max(0.0),
                121 => state.scroll_y += bounds.size.height,
                115 => state.scroll_y = 0.0,
                119 => {
                    state.scroll_y = state
                        .layout
                        .as_ref()
                        .map(|layout| layout.content_height)
                        .unwrap_or(0.0)
                }
                _ => {
                    drop(state);
                    // SAFETY: Unhandled keys retain NSView's responder behavior.
                    unsafe {
                        let _: () = msg_send![super(self), keyDown: event];
                    }
                    return;
                }
            }
            state.clamp_scroll(bounds.size.height);
            drop(state);
            self.sync_native_scroll_to_state();
            self.render_frame();
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: &AnyObject) {
            self.copy_selection();
        }

        #[unsafe(method(selectAll:))]
        fn select_all_action(&self, _sender: &AnyObject) {
            self.select_all();
        }
    }
);

impl DiffMetalView {
    pub fn new(frame: NSRect, font_size: f64, mtm: MainThreadMarker) -> Retained<Self> {
        let (font_size, line_height, char_width) = diff_metrics(font_size);
        let this = Self::alloc(mtm).set_ivars(DiffMetalViewIvars {
            metal: RefCell::new(None),
            state: RefCell::new(DiffState {
                font_size,
                line_height,
                char_width,
                ..DiffState::default()
            }),
            dragging_selection: Cell::new(false),
            search_field: RefCell::new(None),
            search_panel: RefCell::new(None),
            tracking_area: RefCell::new(None),
            scrollbar_hovered: Cell::new(false),
            scrollbar_active: Cell::new(false),
            scrollbar_hover_progress: Cell::new(0.0),
            scrollbar_drag_start_y: Cell::new(0.0),
            scrollbar_drag_start_scroll: Cell::new(0.0),
            display_link: RefCell::new(None),
            window_occluded: Cell::new(false),
            last_frame_timestamp: Cell::new(0.0),
            native_scroll: RefCell::new(None),
            native_scroll_document: RefCell::new(None),
        });
        // SAFETY: NSView's designated frame initializer is valid for this subclass.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.initialize_metal();
        view.initialize_tracking();
        view.initialize_native_scroll();
        view.setAccessibilityRole(Some(unsafe { NSAccessibilityTextAreaRole }));
        view.setAccessibilityLabel(Some(&NSString::from_str("File diff")));
        view.update_accessibility_content();
        view
    }

    pub fn set_font_size(&self, font_size: f64) {
        let (font_size, line_height, char_width) = diff_metrics(font_size);
        let bounds = self.bounds();
        let mut state = self.ivars().state.borrow_mut();
        if (state.font_size - font_size).abs() < f64::EPSILON {
            return;
        }
        state.font_size = font_size;
        state.line_height = line_height;
        state.char_width = char_width;
        state.layout = None;
        state.layout_width = 0;
        state.ensure_layout(bounds.size.width, bounds.size.height);
        drop(state);
        self.update_native_scroll_geometry();
        self.sync_native_scroll_to_state();
        self.render_frame();
        log::debug!(
            "native diff font metrics updated font_size={font_size} line_height={line_height} char_width={char_width}"
        );
    }

    pub fn set_document(
        &self,
        path: &str,
        fingerprint: u64,
        document: DiffDocument,
        syntax: Vec<DiffSyntaxSpan>,
    ) {
        let bounds = self.bounds();
        let mut state = self.ivars().state.borrow_mut();
        state.set_document(path, fingerprint, document, syntax);
        state.ensure_layout(bounds.size.width, bounds.size.height);
        log::info!(
            "native Skia Metal diff updated path={} rows={} folds={} fingerprint={:016x}",
            path,
            state.full_rows.len(),
            state.folds.len(),
            fingerprint
        );
        drop(state);
        self.update_native_scroll_geometry();
        self.sync_native_scroll_to_state();
        self.update_accessibility_content();
        self.render_frame();
    }

    pub fn clear(&self) {
        self.ivars().state.borrow_mut().clear();
        self.update_native_scroll_geometry();
        self.sync_native_scroll_to_state();
        self.update_accessibility_content();
        self.render_frame();
    }

    pub fn attach_search_panel(&self, panel: &NSView, field: &NSSearchField) {
        self.ivars().search_panel.replace(Some(panel.retain()));
        self.ivars().search_field.replace(Some(field.retain()));
    }

    pub fn set_search_query(&self, query: &str) {
        let bounds = self.bounds();
        let mut state = self.ivars().state.borrow_mut();
        if state.search_query == query {
            return;
        }
        state.search_query = query.to_string();
        state.rebuild_search_matches();
        if state.active_search_match.is_some() {
            state.select_search_match(0, bounds.size.height);
        }
        drop(state);
        self.sync_native_scroll_to_state();
        self.update_accessibility_selection();
        self.render_frame();
    }

    pub fn search_next(&self) {
        let height = self.bounds().size.height;
        self.ivars()
            .state
            .borrow_mut()
            .select_search_match(1, height);
        self.sync_native_scroll_to_state();
        self.update_accessibility_selection();
        self.render_frame();
    }

    pub fn search_previous(&self) {
        let height = self.bounds().size.height;
        self.ivars()
            .state
            .borrow_mut()
            .select_search_match(-1, height);
        self.sync_native_scroll_to_state();
        self.update_accessibility_selection();
        self.render_frame();
    }

    pub fn search_status(&self) -> String {
        let state = self.ivars().state.borrow();
        if state.search_query.trim().is_empty() {
            String::new()
        } else if state.search_matches.is_empty() {
            "No matches".to_string()
        } else {
            format!(
                "{} of {}",
                state.active_search_match.unwrap_or(0) + 1,
                state.search_matches.len()
            )
        }
    }

    fn initialize_metal(&self) {
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            log::error!("Metal is unavailable; native diff cannot be created");
            return;
        };
        let Some(command_queue) = device.newCommandQueue() else {
            log::error!("Metal command queue creation failed");
            return;
        };
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setPresentsWithTransaction(false);
        layer.setFramebufferOnly(false);
        layer.setMaximumDrawableCount(3);
        let backend = unsafe {
            mtl::BackendContext::new(
                Retained::as_ptr(&device) as mtl::Handle,
                Retained::as_ptr(&command_queue) as mtl::Handle,
            )
        };
        let Some(skia) = gpu::direct_contexts::make_metal(&backend, None) else {
            log::error!("Skia Metal direct context creation failed");
            return;
        };
        self.setLayer(Some(&layer));
        self.setWantsLayer(true);
        self.ivars().metal.replace(Some(MetalState {
            _device: device,
            layer,
            command_queue,
            skia,
        }));
        log::info!("native Skia Metal diff surface initialized");
    }

    pub fn teardown_renderer(&self) {
        if let Some(display_link) = self.ivars().display_link.borrow_mut().take() {
            display_link.setPaused(true);
            display_link.setDelegate(None);
            display_link.invalidate();
        }
        self.ivars().last_frame_timestamp.set(0.0);
        self.setLayer(None);
        self.setWantsLayer(false);
        self.ivars().metal.borrow_mut().take();
    }

    pub fn set_window_occluded(&self, occluded: bool) {
        if self.ivars().window_occluded.replace(occluded) == occluded {
            return;
        }
        self.refresh_renderer_visibility();
        log::debug!("native diff renderer occluded={occluded}");
    }

    pub fn refresh_renderer_visibility(&self) {
        if self.can_render() {
            self.render_frame();
        } else if let Some(display_link) = self.ivars().display_link.borrow().as_ref() {
            display_link.setPaused(true);
            self.ivars().last_frame_timestamp.set(0.0);
        }
    }

    fn initialize_tracking(&self) {
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::ActiveInActiveApp
                    | NSTrackingAreaOptions::InVisibleRect
                    | NSTrackingAreaOptions::EnabledDuringMouseDrag,
                Some(self),
                None,
            )
        };
        self.addTrackingArea(&tracking);
        self.ivars().tracking_area.replace(Some(tracking));
    }

    fn initialize_native_scroll(&self) {
        let bounds = self.bounds();
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new((bounds.size.width - 1.0).max(0.0), 0.0),
                NSSize::new(1.0, bounds.size.height.max(1.0)),
            ),
        );
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setAlphaValue(0.0);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(false);
        scroll.setHasHorizontalScroller(false);
        scroll.setVerticalScrollElasticity(NSScrollElasticity::Allowed);
        scroll.setHorizontalScrollElasticity(NSScrollElasticity::None);

        let document = NativeScrollDocument::new(
            NSRect::new(NSPoint::ZERO, NSSize::new(1.0, bounds.size.height.max(1.0))),
            self.mtm(),
        );
        scroll.setDocumentView(Some(&document));
        let clip = scroll.contentView();
        clip.setDrawsBackground(false);
        clip.setPostsBoundsChangedNotifications(true);
        // SAFETY: The observed clip view and this retained diff view share the same AppKit
        // lifetime, and bounds-change delivery is confined to the main thread.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                objc2::sel!(nativeScrollBoundsChanged:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
            );
        }
        self.addSubview(&scroll);
        self.ivars().native_scroll.replace(Some(scroll));
        self.ivars().native_scroll_document.replace(Some(document));
        self.update_native_scroll_geometry();
        log::debug!("native diff scroll physics attached through hidden one-point NSScrollView");
    }

    fn update_native_scroll_geometry(&self) {
        let bounds = self.bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let content_height = {
            let mut state = self.ivars().state.borrow_mut();
            state.ensure_layout(bounds.size.width, bounds.size.height);
            state
                .layout
                .as_ref()
                .map_or(bounds.size.height, |layout| layout.content_height)
                .max(bounds.size.height)
        };
        if let Some(scroll) = self.ivars().native_scroll.borrow().as_ref() {
            scroll.setFrame(NSRect::new(
                NSPoint::new((bounds.size.width - 1.0).max(0.0), 0.0),
                NSSize::new(1.0, bounds.size.height),
            ));
        }
        if let Some(document) = self.ivars().native_scroll_document.borrow().as_ref() {
            let frame = document.frame();
            if (frame.size.width - 1.0).abs() > 0.5
                || (frame.size.height - content_height).abs() > 0.5
            {
                document.setFrameSize(NSSize::new(1.0, content_height));
            }
        }
    }

    fn apply_native_scroll_position(&self) {
        let Some(scroll) = self.ivars().native_scroll.borrow().as_ref().cloned() else {
            return;
        };
        let bounds = self.bounds();
        let scroll_y = scroll.contentView().bounds().origin.y;
        let mut state = self.ivars().state.borrow_mut();
        let maximum = state.maximum_scroll(bounds.size.height);
        state.scroll_y = scroll_y;
        state.elastic_scrolling = scroll_y < 0.0 || scroll_y > maximum;
        drop(state);
        self.render_frame();
    }

    fn sync_native_scroll_to_state(&self) {
        self.update_native_scroll_geometry();
        let Some(scroll) = self.ivars().native_scroll.borrow().as_ref().cloned() else {
            return;
        };
        let bounds = self.bounds();
        let scroll_y = {
            let state = self.ivars().state.borrow();
            state
                .scroll_y
                .clamp(0.0, state.maximum_scroll(bounds.size.height))
        };
        let clip = scroll.contentView();
        clip.scrollToPoint(NSPoint::new(0.0, scroll_y));
        scroll.reflectScrolledClipView(&clip);
    }

    fn initialize_display_link(&self) {
        if self.ivars().display_link.borrow().is_some() || self.window().is_none() {
            return;
        }
        let Some(metal) = self
            .ivars()
            .metal
            .borrow()
            .as_ref()
            .map(|metal| metal.layer.clone())
        else {
            return;
        };
        let display_link =
            CAMetalDisplayLink::initWithMetalLayer(CAMetalDisplayLink::alloc(), &metal);
        display_link.setDelegate(Some(ProtocolObject::from_ref(self)));
        display_link.setPreferredFrameLatency(1.0);
        display_link.setPaused(true);
        let main_run_loop = NSRunLoop::mainRunLoop();
        unsafe {
            display_link.addToRunLoop_forMode(&main_run_loop, NSRunLoopCommonModes);
            display_link.addToRunLoop_forMode(&main_run_loop, NSEventTrackingRunLoopMode);
        }
        self.ivars().display_link.replace(Some(display_link));
        log::debug!("native Metal display link attached to its visible diff view");
    }

    fn point_in_scrollbar_lane(&self, point: NSPoint) -> bool {
        let bounds = self.bounds();
        point.x >= bounds.size.width - SCROLLBAR_WIDTH
            && point.x <= bounds.size.width
            && point.y >= 0.0
            && point.y <= bounds.size.height
    }

    fn set_scrollbar_hovered(&self, hovered: bool) {
        if self.ivars().scrollbar_hovered.replace(hovered) == hovered {
            return;
        }
        self.start_display_link();
    }

    fn start_display_link(&self) {
        if !self.can_render() {
            return;
        }
        if self.ivars().display_link.borrow().is_none() {
            self.initialize_display_link();
        }
        self.ivars().last_frame_timestamp.set(0.0);
        if let Some(display_link) = self.ivars().display_link.borrow().as_ref() {
            display_link.setPaused(false);
        }
    }

    fn advance_animation(
        &self,
        display_link: &CAMetalDisplayLink,
        update: &CAMetalDisplayLinkUpdate,
    ) {
        let timestamp = update.targetTimestamp();
        let previous = self.ivars().last_frame_timestamp.replace(timestamp);
        let elapsed = if previous > 0.0 {
            (timestamp - previous).clamp(1.0 / 240.0, 0.1)
        } else {
            1.0 / 60.0
        };
        let mut animating = false;
        let target = if self.ivars().scrollbar_hovered.get() || self.ivars().scrollbar_active.get()
        {
            1.0
        } else {
            0.0
        };
        let current = self.ivars().scrollbar_hover_progress.get();
        let delta = target - current;
        if delta.abs() < 0.02 {
            self.ivars().scrollbar_hover_progress.set(target);
        } else {
            animating = true;
            let step = elapsed / SCROLLBAR_ANIMATION_DURATION_SECONDS;
            self.ivars()
                .scrollbar_hover_progress
                .set((current + delta.clamp(-step, step)).clamp(0.0, 1.0));
        }

        self.render_drawable(&update.drawable(), false);
        if !animating {
            display_link.setPaused(true);
            self.ivars().last_frame_timestamp.set(0.0);
        }
    }

    fn render_frame(&self) {
        if !self.can_render() {
            return;
        }
        if self.ivars().display_link.borrow().is_none() {
            self.initialize_display_link();
        }
        if let Some(display_link) = self.ivars().display_link.borrow().as_ref() {
            display_link.setPaused(false);
            return;
        }
        let drawable = {
            let metal = self.ivars().metal.borrow();
            let Some(metal) = metal.as_ref() else {
                return;
            };
            let Some(drawable) = metal.layer.nextDrawable() else {
                log::warn!("native diff skipped frame because no Metal drawable was available");
                return;
            };
            drawable
        };
        self.render_drawable(&drawable, true);
    }

    fn can_render(&self) -> bool {
        self.window().is_some()
            && !self.ivars().window_occluded.get()
            && !self.isHiddenOrHasHiddenAncestor()
    }

    fn render_drawable(
        &self,
        drawable: &ProtocolObject<dyn CAMetalDrawable>,
        present_with_command_buffer: bool,
    ) {
        let bounds = self.bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let backing = self.convertRectToBacking(bounds);
        let pixel_width = backing.size.width.max(1.0).round() as i32;
        let pixel_height = backing.size.height.max(1.0).round() as i32;
        let scale_x = pixel_width as f32 / bounds.size.width as f32;
        let scale_y = pixel_height as f32 / bounds.size.height as f32;

        let mut metal_ref = self.ivars().metal.borrow_mut();
        let Some(metal) = metal_ref.as_mut() else {
            return;
        };
        metal.layer.setContentsScale(scale_x as f64);
        metal
            .layer
            .setDrawableSize(CGSize::new(pixel_width as f64, pixel_height as f64));
        let texture = drawable.texture();
        let texture_info =
            unsafe { mtl::TextureInfo::new(Retained::as_ptr(&texture) as mtl::Handle) };
        let backend_target =
            backend_render_targets::make_mtl((pixel_width, pixel_height), &texture_info);
        let Some(mut surface) = gpu::surfaces::wrap_backend_render_target(
            &mut metal.skia,
            &backend_target,
            SurfaceOrigin::TopLeft,
            ColorType::BGRA8888,
            None,
            None,
        ) else {
            log::error!("Skia failed to wrap the current Metal drawable");
            return;
        };
        let canvas = surface.canvas();
        canvas.scale((scale_x, scale_y));
        let mut state = self.ivars().state.borrow_mut();
        state.ensure_layout(bounds.size.width, bounds.size.height);
        if let Some(layout) = state.layout.as_ref() {
            paint_diff(
                canvas,
                DiffPaintRequest {
                    rows: &state.rows,
                    layout,
                    viewport_width: (bounds.size.width - SCROLLBAR_WIDTH).max(1.0) as f32,
                    viewport_height: bounds.size.height as f32,
                    scroll_y: state.scroll_y,
                    selection: state.selection,
                    search_matches: &state.search_matches,
                    active_search_match: state.active_search_match,
                    syntax: &state.syntax,
                    char_width: state.char_width as f32,
                    font_size: state.font_size as f32,
                    baseline_offset: (state.font_size * BASELINE_OFFSET_RATIO) as f32,
                },
            );
            draw_skia_scrollbar(
                canvas,
                bounds.size.width,
                bounds.size.height,
                layout,
                state.scroll_y,
                self.ivars().scrollbar_hover_progress.get(),
                self.ivars().scrollbar_active.get(),
            );
        }
        drop(state);
        metal.skia.flush_and_submit();
        drop(surface);
        let Some(command_buffer) = metal.command_queue.commandBuffer() else {
            log::error!("Metal command buffer creation failed");
            return;
        };
        if present_with_command_buffer {
            let presentable: &ProtocolObject<dyn MTLDrawable> = drawable.as_ref();
            command_buffer.presentDrawable(presentable);
            command_buffer.commit();
        } else {
            command_buffer.commit();
            drawable.present();
        }
    }

    pub fn focus_search(&self) {
        if let Some(panel) = self.ivars().search_panel.borrow().as_ref() {
            panel.setHidden(false);
        }
        let Some(field) = self.ivars().search_field.borrow().clone() else {
            return;
        };
        field.setHidden(false);
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(&field));
        }
    }

    fn select_all(&self) {
        let mut state = self.ivars().state.borrow_mut();
        let side = state.active_side.unwrap_or(DiffSide::Right);
        let Some(selection) = select_all_diff_text(&state.rows, side) else {
            return;
        };
        state.selection = Some(selection);
        drop(state);
        self.update_accessibility_selection();
        self.render_frame();
    }

    fn copy_selection(&self) {
        let state = self.ivars().state.borrow();
        let output = state.selected_text();
        drop(state);
        let Some(output) = output.filter(|output| !output.is_empty()) else {
            return;
        };
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(&output), unsafe {
            NSPasteboardTypeString
        });
    }

    fn update_accessibility_content(&self) {
        let state = self.ivars().state.borrow();
        let value = state.accessibility_text();
        let label = if state.path.is_empty() {
            "File diff".to_string()
        } else {
            format!("File diff for {}", state.path)
        };
        drop(state);
        self.setAccessibilityLabel(Some(&NSString::from_str(&label)));
        // SAFETY: NSString is a valid accessibility value and AppKit copies/retains it as needed.
        unsafe { self.setAccessibilityValue(Some(&NSString::from_str(&value))) };
        self.update_accessibility_selection();
    }

    fn update_accessibility_selection(&self) {
        let selection = self.ivars().state.borrow().selected_text();
        self.setAccessibilitySelectedText(selection.as_deref().map(NSString::from_str).as_deref());
    }
}

impl DiffState {
    fn selected_text(&self) -> Option<String> {
        selected_diff_text(&self.rows, self.selection?)
    }

    fn accessibility_text(&self) -> String {
        if self.full_rows.is_empty() {
            return if self.path.is_empty() {
                "No diff loaded".to_string()
            } else {
                format!("No changes in {}", self.path)
            };
        }
        let mut output = String::with_capacity(
            self.full_rows
                .len()
                .saturating_mul(48)
                .min(DIFF_ACCESSIBILITY_MAX_BYTES),
        );
        if !self.path.is_empty() {
            output.push_str(&self.path);
            output.push('\n');
        }
        let mut truncated = false;
        for row in &self.full_rows {
            let left_deleted = row.left_kind == DiffRowKind::Deleted;
            let right_added = row.right_kind == DiffRowKind::Added;
            let entries = if left_deleted || right_added {
                [
                    left_deleted.then_some(("-", row.left_number, row.left_text.as_deref())),
                    right_added.then_some(("+", row.right_number, row.right_text.as_deref())),
                ]
            } else {
                [
                    Some((
                        " ",
                        row.right_number.or(row.left_number),
                        row.right_text.as_deref().or(row.left_text.as_deref()),
                    )),
                    None,
                ]
            };
            for entry in entries.into_iter().flatten() {
                let line = format!(
                    "{} {}: {}\n",
                    entry.0,
                    entry
                        .1
                        .map_or_else(|| "?".to_string(), |line| line.to_string()),
                    entry.2.unwrap_or_default()
                );
                if output.len().saturating_add(line.len()) > DIFF_ACCESSIBILITY_MAX_BYTES {
                    truncated = true;
                    break;
                }
                output.push_str(&line);
            }
            if truncated {
                break;
            }
        }
        if truncated {
            output.push_str("… Diff content truncated for accessibility.");
        }
        output
    }

    fn row_at_point(&self, point: NSPoint) -> Option<usize> {
        diff_row_index_at_y(self.layout.as_ref()?, self.scroll_y + point.y)
    }

    fn text_point_at(&self, point: NSPoint, width: f64) -> Option<DiffTextPoint> {
        let row_index = self.row_at_point(point)?;
        let row = self.rows.get(row_index)?;
        if row.left_kind == DiffRowKind::Fold || row.right_kind == DiffRowKind::Fold {
            return None;
        }
        let layout = self.layout.as_ref()?.rows.get(row_index)?;
        let side = if point.x < width / 2.0 {
            DiffSide::Left
        } else {
            DiffSide::Right
        };
        let (lines, text, text_x) = match side {
            DiffSide::Left => (&layout.left_lines, row.left_text.as_deref()?, CELL_PADDING),
            DiffSide::Right => (
                &layout.right_lines,
                row.right_text.as_deref()?,
                width / 2.0 + 1.0 + GUTTER_WIDTH + CELL_PADDING,
            ),
        };
        let local_y = (self.scroll_y + point.y - layout.y).max(0.0);
        let line_index =
            ((local_y / self.line_height).floor() as usize).min(lines.len().saturating_sub(1));
        let line = lines.get(line_index)?;
        let column = ((point.x - text_x).max(0.0) / self.char_width).round() as usize;
        let local_byte = byte_offset_for_column(&line.text, column);
        let byte = (line.start + local_byte).min(line.end).min(text.len());
        Some(DiffTextPoint {
            side,
            row: row_index,
            byte,
        })
    }

    fn selection_focus_at(
        &self,
        point: NSPoint,
        width: f64,
        side: DiffSide,
    ) -> Option<DiffTextPoint> {
        let row_index = self.row_at_point(point)?;
        let row = self.rows.get(row_index)?;
        let text = match side {
            DiffSide::Left => row.left_text.as_deref(),
            DiffSide::Right => row.right_text.as_deref(),
        };
        let Some(text) = text else {
            return Some(DiffTextPoint {
                side,
                row: row_index,
                byte: 0,
            });
        };
        if row.left_kind == DiffRowKind::Fold || row.right_kind == DiffRowKind::Fold {
            return Some(DiffTextPoint {
                side,
                row: row_index,
                byte: 0,
            });
        }
        let layout = self.layout.as_ref()?.rows.get(row_index)?;
        let lines = match side {
            DiffSide::Left => &layout.left_lines,
            DiffSide::Right => &layout.right_lines,
        };
        if lines.is_empty() {
            return Some(DiffTextPoint {
                side,
                row: row_index,
                byte: 0,
            });
        }
        let text_x = match side {
            DiffSide::Left => CELL_PADDING,
            DiffSide::Right => width / 2.0 + 1.0 + GUTTER_WIDTH + CELL_PADDING,
        };
        let local_y = (self.scroll_y + point.y - layout.y).max(0.0);
        let line_index =
            ((local_y / self.line_height).floor() as usize).min(lines.len().saturating_sub(1));
        let line = lines.get(line_index)?;
        let column = ((point.x - text_x).max(0.0) / self.char_width).round() as usize;
        let byte = (line.start + byte_offset_for_column(&line.text, column))
            .min(line.end)
            .min(text.len());
        Some(DiffTextPoint {
            side,
            row: row_index,
            byte,
        })
    }
}

fn byte_offset_for_column(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

fn diff_metrics(font_size: f64) -> (f64, f64, f64) {
    let font_size = font_size.clamp(8.0, 32.0);
    let scale = font_size / BASE_FONT_SIZE;
    (
        font_size,
        (BASE_LINE_HEIGHT * scale).max(1.0),
        (BASE_CHAR_WIDTH * scale).max(1.0),
    )
}
