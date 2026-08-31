use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSBorderType, NSColor, NSEvent, NSImage, NSScrollView, NSView};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize};
use std::cell::{Cell, OnceCell, RefCell};

const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 16.0;
const ZOOM_STEP: f64 = 1.1;
const CHECKER_SIZE: f64 = 16.0;

struct ImageCanvasIvars {
    image: RefCell<Option<Retained<NSImage>>>,
    source_size: Cell<NSSize>,
    fit_scale: Cell<f64>,
    user_scale: Cell<f64>,
    fit_mode: Cell<bool>,
    drag_start: Cell<Option<(NSPoint, NSPoint)>>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ImageCanvasIvars]
    struct ImageCanvas;

    unsafe impl NSObjectProtocol for ImageCanvas {}

    impl ImageCanvas {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            let start_column = (dirty.origin.x / CHECKER_SIZE).floor() as isize;
            let end_column = ((dirty.origin.x + dirty.size.width) / CHECKER_SIZE).ceil() as isize;
            let start_row = (dirty.origin.y / CHECKER_SIZE).floor() as isize;
            let end_row = ((dirty.origin.y + dirty.size.height) / CHECKER_SIZE).ceil() as isize;
            let light = NSColor::colorWithSRGBRed_green_blue_alpha(0.98, 0.98, 0.98, 1.0);
            let dark = NSColor::colorWithSRGBRed_green_blue_alpha(0.87, 0.87, 0.87, 1.0);
            for row in start_row..end_row {
                for column in start_column..end_column {
                    if (row + column).rem_euclid(2) == 0 {
                        light.setFill();
                    } else {
                        dark.setFill();
                    }
                    NSBezierPath::fillRect(NSRect::new(
                        NSPoint::new(
                            column as f64 * CHECKER_SIZE,
                            row as f64 * CHECKER_SIZE,
                        ),
                        NSSize::new(CHECKER_SIZE, CHECKER_SIZE),
                    ));
                }
            }

            let Some(image) = self.ivars().image.borrow().clone() else {
                return;
            };
            let display = self.display_size();
            let bounds = self.bounds();
            let origin = NSPoint::new(
                ((bounds.size.width - display.width) / 2.0).max(0.0),
                ((bounds.size.height - display.height) / 2.0).max(0.0),
            );
            image.drawInRect(NSRect::new(origin, display));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if self.ivars().image.borrow().is_none() {
                unsafe { let _: () = msg_send![super(self), scrollWheel: event]; }
                return;
            }
            let delta = event.scrollingDeltaY();
            if delta.abs() <= f64::EPSILON {
                unsafe { let _: () = msg_send![super(self), scrollWheel: event]; }
                return;
            }
            let pointer = self.convertPoint_fromView(event.locationInWindow(), None);
            let target = if delta > 0.0 {
                self.current_scale() * ZOOM_STEP
            } else {
                self.current_scale() / ZOOM_STEP
            };
            self.apply_zoom(target, Some(pointer));
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if self.ivars().image.borrow().is_none() {
                return;
            }
            if event.clickCount() == 2 {
                let pointer = self.convertPoint_fromView(event.locationInWindow(), None);
                if self.ivars().fit_mode.get() {
                    self.apply_zoom(self.ivars().fit_scale.get() * ZOOM_STEP * ZOOM_STEP, Some(pointer));
                } else {
                    self.apply_fit();
                }
                return;
            }
            let Some(scroll) = self.enclosingScrollView() else {
                return;
            };
            self.ivars().drag_start.set(Some((
                event.locationInWindow(),
                scroll.contentView().bounds().origin,
            )));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let Some((pointer, origin)) = self.ivars().drag_start.get() else {
                return;
            };
            let current = event.locationInWindow();
            self.scroll_to(NSPoint::new(
                origin.x - (current.x - pointer.x),
                origin.y - (current.y - pointer.y),
            ));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().drag_start.set(None);
        }
    }
);

impl ImageCanvas {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ImageCanvasIvars {
            image: RefCell::new(None),
            source_size: Cell::new(NSSize::new(0.0, 0.0)),
            fit_scale: Cell::new(1.0),
            user_scale: Cell::new(1.0),
            fit_mode: Cell::new(true),
            drag_start: Cell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn set_image(&self, image: &NSImage) {
        let pixel_size = image.representations().iter().fold(
            NSSize::new(0.0, 0.0),
            |largest, representation| {
                let candidate = NSSize::new(
                    representation.pixelsWide() as f64,
                    representation.pixelsHigh() as f64,
                );
                if candidate.width * candidate.height > largest.width * largest.height {
                    candidate
                } else {
                    largest
                }
            },
        );
        let size = if pixel_size.width > 0.0 && pixel_size.height > 0.0 {
            pixel_size
        } else {
            image.size()
        };
        if size.width <= 0.0 || size.height <= 0.0 {
            self.clear_image();
            return;
        }
        self.ivars().image.replace(Some(image.retain()));
        self.ivars().source_size.set(size);
        self.ivars().fit_mode.set(true);
        self.recalculate_fit();
    }

    fn clear_image(&self) {
        self.ivars().image.borrow_mut().take();
        self.ivars().source_size.set(NSSize::new(0.0, 0.0));
        self.ivars().fit_scale.set(1.0);
        self.ivars().user_scale.set(1.0);
        self.ivars().fit_mode.set(true);
        self.ivars().drag_start.set(None);
        let viewport = self.viewport_size();
        self.setFrameSize(NSSize::new(
            viewport.width.max(1.0),
            viewport.height.max(1.0),
        ));
        self.scroll_to(NSPoint::ZERO);
        self.setNeedsDisplay(true);
    }

    fn recalculate_fit(&self) {
        let size = self.ivars().source_size.get();
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        let viewport = self.viewport_size();
        let old_scale = self.current_scale();
        let fit = if viewport.width <= 0.0 || viewport.height <= 0.0 {
            1.0
        } else {
            (viewport.width / size.width).min(viewport.height / size.height)
        };
        let was_fit = self.ivars().fit_mode.get();
        self.ivars().fit_scale.set(fit);
        if was_fit || old_scale <= fit {
            self.ivars().fit_mode.set(true);
            self.ivars().user_scale.set(1.0);
        } else {
            self.ivars().fit_mode.set(false);
            self.ivars()
                .user_scale
                .set((old_scale / fit).clamp(MIN_ZOOM, MAX_ZOOM));
        }
        self.resize_document();
        if self.ivars().fit_mode.get() {
            self.scroll_to(NSPoint::ZERO);
        }
    }

    fn apply_fit(&self) {
        self.ivars().fit_mode.set(true);
        self.ivars().user_scale.set(1.0);
        self.resize_document();
        self.scroll_to(NSPoint::ZERO);
    }

    fn apply_zoom(&self, target: f64, pointer_document: Option<NSPoint>) {
        let Some(scroll) = self.enclosingScrollView() else {
            return;
        };
        let viewport = scroll.contentSize();
        if viewport.width <= 0.0 || viewport.height <= 0.0 {
            return;
        }
        let old_scale = self.effective_scale();
        let target = target.clamp(MIN_ZOOM, MAX_ZOOM);
        let fit = self.ivars().fit_scale.get();
        let new_scale = if target <= fit {
            self.ivars().fit_mode.set(true);
            self.ivars().user_scale.set(1.0);
            fit.clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            self.ivars().fit_mode.set(false);
            self.ivars().user_scale.set(target / fit);
            target.clamp(MIN_ZOOM, MAX_ZOOM)
        };
        if (old_scale - new_scale).abs() <= f64::EPSILON {
            return;
        }

        let old_display = self.display_size_for_scale(old_scale);
        let old_bounds = self.bounds();
        let old_image_origin = NSPoint::new(
            ((old_bounds.size.width - old_display.width) / 2.0).max(0.0),
            ((old_bounds.size.height - old_display.height) / 2.0).max(0.0),
        );
        let clip_origin = scroll.contentView().bounds().origin;
        let pointer_document = pointer_document.unwrap_or(NSPoint::new(
            clip_origin.x + viewport.width / 2.0,
            clip_origin.y + viewport.height / 2.0,
        ));
        let pointer_viewport = NSPoint::new(
            pointer_document.x - clip_origin.x,
            pointer_document.y - clip_origin.y,
        );
        let ratio_x =
            ((pointer_document.x - old_image_origin.x) / old_display.width).clamp(0.0, 1.0);
        let ratio_y =
            ((pointer_document.y - old_image_origin.y) / old_display.height).clamp(0.0, 1.0);

        self.resize_document();
        let new_display = self.display_size();
        let new_bounds = self.bounds();
        let new_image_origin = NSPoint::new(
            ((new_bounds.size.width - new_display.width) / 2.0).max(0.0),
            ((new_bounds.size.height - new_display.height) / 2.0).max(0.0),
        );
        self.scroll_to(NSPoint::new(
            new_image_origin.x + ratio_x * new_display.width - pointer_viewport.x,
            new_image_origin.y + ratio_y * new_display.height - pointer_viewport.y,
        ));
    }

    fn resize_document(&self) {
        let viewport = self.viewport_size();
        let display = self.display_size();
        self.setFrameSize(NSSize::new(
            viewport.width.max(display.width).max(1.0),
            viewport.height.max(display.height).max(1.0),
        ));
        self.setNeedsDisplay(true);
    }

    fn scroll_to(&self, point: NSPoint) {
        let Some(scroll) = self.enclosingScrollView() else {
            return;
        };
        let viewport = scroll.contentSize();
        let bounds = self.bounds();
        let point = NSPoint::new(
            point
                .x
                .clamp(0.0, (bounds.size.width - viewport.width).max(0.0)),
            point
                .y
                .clamp(0.0, (bounds.size.height - viewport.height).max(0.0)),
        );
        let clip = scroll.contentView();
        clip.scrollToPoint(point);
        scroll.reflectScrolledClipView(&clip);
    }

    fn display_size(&self) -> NSSize {
        self.display_size_for_scale(self.effective_scale())
    }

    fn current_scale(&self) -> f64 {
        let fit = self.ivars().fit_scale.get();
        if self.ivars().fit_mode.get() {
            fit
        } else {
            fit * self.ivars().user_scale.get()
        }
    }

    fn effective_scale(&self) -> f64 {
        self.current_scale().clamp(MIN_ZOOM, MAX_ZOOM)
    }

    fn display_size_for_scale(&self, scale: f64) -> NSSize {
        let source = self.ivars().source_size.get();
        NSSize::new(source.width * scale, source.height * scale)
    }

    fn viewport_size(&self) -> NSSize {
        self.enclosingScrollView()
            .map_or_else(|| self.bounds().size, |scroll| scroll.contentSize())
    }
}

pub(crate) struct NativeImagePreviewIvars {
    canvas: OnceCell<Retained<ImageCanvas>>,
}

define_class!(
    #[unsafe(super = NSScrollView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = NativeImagePreviewIvars]
    pub(crate) struct NativeImagePreview;

    unsafe impl NSObjectProtocol for NativeImagePreview {}
);

impl NativeImagePreview {
    pub(crate) fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NativeImagePreviewIvars {
            canvas: OnceCell::new(),
        });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setBorderType(NSBorderType::NoBorder);
        view.setDrawsBackground(false);
        view.setHasHorizontalScroller(true);
        view.setHasVerticalScroller(true);
        view.setAutohidesScrollers(true);
        let canvas = ImageCanvas::new(NSRect::new(NSPoint::ZERO, frame.size), mtm);
        view.setDocumentView(Some(&canvas));
        let _ = view.ivars().canvas.set(canvas);
        view
    }

    pub(crate) fn set_image(&self, image: &NSImage) {
        if let Some(canvas) = self.ivars().canvas.get() {
            canvas.set_image(image);
        }
    }

    pub(crate) fn clear_image(&self) {
        if let Some(canvas) = self.ivars().canvas.get() {
            canvas.clear_image();
        }
    }

    pub(crate) fn recalculate_fit(&self) {
        if let Some(canvas) = self.ivars().canvas.get() {
            canvas.recalculate_fit();
        }
    }
}
