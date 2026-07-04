mod loader;
mod zoom;

use gtk::glib;
use gtk::{gdk, gio, prelude::*};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

use crate::ui::widgets;

pub(in crate::ui::pages::file) struct ImageViewer {
    pub(in crate::ui::pages::file) root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    picture: gtk::Picture,
    status: gtk::Label,
    state: ImageViewerState,
}

struct ImageViewerState {
    load_token: Cell<u64>,
    source_size: RefCell<Option<(i32, i32)>>,
    fit_scale: Cell<f64>,
    user_scale: Cell<f64>,
    fit_mode: Cell<bool>,
    pointer_position: RefCell<Option<(f64, f64)>>,
    drag_origin: RefCell<Option<(f64, f64)>>,
}

const PRELOAD_POLL_MS: u64 = 16;

impl ImageViewer {
    pub(in crate::ui::pages::file) fn new() -> Rc<Self> {
        let checkerboard = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        checkerboard.set_draw_func(|_, cr, width, height| {
            let cell = 16.0_f64;
            let light = (0.98_f64, 0.98_f64, 0.98_f64);
            let dark = (0.87_f64, 0.87_f64, 0.87_f64);
            let cols = (width as f64 / cell).ceil() as i32 + 1;
            let rows = (height as f64 / cell).ceil() as i32 + 1;

            for row in 0..rows {
                for col in 0..cols {
                    if (row + col) % 2 == 0 {
                        cr.set_source_rgb(light.0, light.1, light.2);
                    } else {
                        cr.set_source_rgb(dark.0, dark.1, dark.2);
                    }
                    cr.rectangle((col as f64) * cell, (row as f64) * cell, cell, cell);
                    let _ = cr.fill();
                }
            }
        });

        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .content_fit(gtk::ContentFit::Contain)
            .build();
        picture.set_visible(false);
        picture.set_overflow(gtk::Overflow::Hidden);

        let status = widgets::muted("No image");
        status.set_halign(gtk::Align::Center);
        status.set_valign(gtk::Align::Center);
        status.set_visible(false);

        let canvas = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        canvas.append(&picture);
        canvas.append(&status);

        let scroller = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&canvas)
            .build();
        scroller.set_has_frame(false);

        let overlay = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        overlay.set_child(Some(&checkerboard));
        overlay.add_overlay(&scroller);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&overlay);

        let viewer = Rc::new(Self {
            root,
            scroller,
            picture,
            status,
            state: ImageViewerState {
                load_token: Cell::new(0),
                source_size: RefCell::new(None),
                fit_scale: Cell::new(1.0),
                user_scale: Cell::new(1.0),
                fit_mode: Cell::new(true),
                pointer_position: RefCell::new(None),
                drag_origin: RefCell::new(None),
            },
        });

        viewer.connect_input();
        viewer
    }

    pub(in crate::ui::pages::file) fn clear(&self) {
        self.state.fit_scale.set(1.0);
        self.state.user_scale.set(1.0);
        self.state.fit_mode.set(true);
        self.state.source_size.replace(None);
        self.state.pointer_position.replace(None);
        self.state.drag_origin.replace(None);
        self.picture.set_size_request(-1, -1);
        self.picture.set_paintable(Option::<&gdk::Paintable>::None);
        self.picture.set_visible(false);
        self.status.set_visible(false);
    }

    pub(in crate::ui::pages::file) fn set_file(self: &Rc<Self>, file_path: &str, file: &gio::File) {
        let token = self.state.load_token.get().wrapping_add(1);
        self.state.load_token.set(token);

        log::debug!("image viewer load started path={file_path} token={token}");

        self.clear();
        self.state.fit_mode.set(true);
        self.state.fit_scale.set(1.0);
        self.state.user_scale.set(1.0);
        self.picture.set_visible(false);
        self.status.set_text("Loading image...");
        self.status.set_visible(true);

        let file = file.clone();
        let file_path = file_path.to_string();
        let log_file_path = file_path.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = loader::load_image(file);
            let _ = sender.send((token, result, file_path));
        });
        self.receive_load(receiver, token, log_file_path);
    }

    fn receive_load(
        self: &Rc<Self>,
        receiver: mpsc::Receiver<(u64, Result<loader::LoadedImage, String>, String)>,
        expected_token: u64,
        expected_path: String,
    ) {
        let viewer = Rc::clone(self);
        glib::timeout_add_local(
            Duration::from_millis(PRELOAD_POLL_MS),
            move || match receiver.try_recv() {
                Ok((token, result, file_path)) => {
                    if token != viewer.state.load_token.get() {
                        log::debug!(
                            "stale image load result ignored path={file_path} token={token} active_token={}",
                            viewer.state.load_token.get()
                        );
                        return glib::ControlFlow::Break;
                    }

                    match result {
                        Ok(load) => viewer.apply_loaded(load),
                        Err(error) => {
                            log::warn!(
                                "image load failed path={file_path} token={token} error={error}"
                            );
                            viewer.show_error(&error);
                            return glib::ControlFlow::Break;
                        }
                    }

                    glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    if viewer.state.load_token.get() == expected_token {
                        viewer.show_error("Image loading failed");
                        log::warn!(
                            "image loading channel disconnected path={expected_path} token={expected_token}"
                        );
                    }
                    glib::ControlFlow::Break
                }
            },
        );
    }

    fn apply_loaded(self: &Rc<Self>, load: loader::LoadedImage) {
        log::debug!(
            "image load completed token={} size={}x{} mime={}",
            self.state.load_token.get(),
            load.width,
            load.height,
            load.mime_type
        );

        self.state
            .source_size
            .replace(Some((load.width, load.height)));
        self.picture.set_paintable(Some(&load.texture));
        self.picture.set_visible(true);
        self.status.set_visible(false);

        let fit_scale = zoom::fit_scale(
            load.width,
            load.height,
            self.scroller.allocated_width(),
            self.scroller.allocated_height(),
        );
        self.state.fit_scale.set(fit_scale);
        self.state.user_scale.set(1.0);
        self.state.fit_mode.set(true);
        self.apply_scale();
    }

    fn apply_scale(&self) {
        let Some((source_width, source_height)) = *self.state.source_size.borrow() else {
            return;
        };

        let scale = zoom::clamp_zoom(self.current_scale());
        let display_width = scale_dimension(source_width, scale);
        let display_height = scale_dimension(source_height, scale);
        self.picture.set_size_request(display_width, display_height);
    }

    fn apply_zoom_with_pointer(self: &Rc<Self>, target_scale: f64, pointer: Option<(f64, f64)>) {
        let old_scale = self.current_scale();
        let Some((source_width, source_height)) = *self.state.source_size.borrow() else {
            return;
        };
        if source_width <= 0 || source_height <= 0 {
            return;
        }

        let (viewport_width, viewport_height) = (
            self.scroller.allocated_width() as f64,
            self.scroller.allocated_height() as f64,
        );
        if viewport_width <= 0.0 || viewport_height <= 0.0 {
            return;
        }

        let old_scale = zoom::clamp_zoom(old_scale);
        let target_scale = zoom::clamp_zoom(target_scale);

        if target_scale <= self.state.fit_scale.get() {
            self.state.fit_mode.set(true);
            self.state.user_scale.set(1.0);
        } else {
            self.state.fit_mode.set(false);
            self.state
                .user_scale
                .set(target_scale / self.state.fit_scale.get());
        }

        let new_scale = self.current_scale();
        if old_scale == new_scale {
            return;
        }

        let pointer = pointer.unwrap_or_else(|| {
            (
                self.scroller.allocated_width() as f64 / 2.0,
                self.scroller.allocated_height() as f64 / 2.0,
            )
        });

        let old_rendered_width = source_width as f64 * old_scale;
        let old_rendered_height = source_height as f64 * old_scale;
        let old_offset_x = ((viewport_width - old_rendered_width) / 2.0).max(0.0);
        let old_offset_y = ((viewport_height - old_rendered_height) / 2.0).max(0.0);

        let content_x = self.scroller.hadjustment().value() + pointer.0 - old_offset_x;
        let content_y = self.scroller.vadjustment().value() + pointer.1 - old_offset_y;
        let ratio_x = (content_x / old_rendered_width).clamp(0.0, 1.0);
        let ratio_y = (content_y / old_rendered_height).clamp(0.0, 1.0);

        self.picture.set_size_request(
            scale_dimension(source_width, new_scale),
            scale_dimension(source_height, new_scale),
        );

        let new_rendered_width = source_width as f64 * new_scale;
        let new_rendered_height = source_height as f64 * new_scale;
        let new_offset_x = ((viewport_width - new_rendered_width) / 2.0).max(0.0);
        let new_offset_y = ((viewport_height - new_rendered_height) / 2.0).max(0.0);
        let max_hscroll = (new_rendered_width - viewport_width).max(0.0);
        let max_vscroll = (new_rendered_height - viewport_height).max(0.0);

        self.scroller.hadjustment().set_value(
            (ratio_x * new_rendered_width + new_offset_x - pointer.0).clamp(0.0, max_hscroll),
        );
        self.scroller.vadjustment().set_value(
            (ratio_y * new_rendered_height + new_offset_y - pointer.1).clamp(0.0, max_vscroll),
        );
    }

    fn current_scale(&self) -> f64 {
        let fit_scale = self.state.fit_scale.get();
        let user_scale = self.state.user_scale.get();

        if self.state.fit_mode.get() {
            fit_scale
        } else {
            fit_scale * user_scale
        }
    }

    fn has_loaded_image(&self) -> bool {
        self.state.source_size.borrow().is_some()
    }

    fn recalculate_fit(self: &Rc<Self>) {
        let Some((source_width, source_height)) = *self.state.source_size.borrow() else {
            return;
        };

        let old_scale = self.current_scale();
        let fit_scale = zoom::fit_scale(
            source_width,
            source_height,
            self.scroller.allocated_width(),
            self.scroller.allocated_height(),
        );
        let was_fit = self.state.fit_mode.get();

        self.state.fit_scale.set(fit_scale);
        if was_fit {
            self.state.user_scale.set(1.0);
            self.state.fit_mode.set(true);
        } else {
            if old_scale <= fit_scale {
                self.state.fit_mode.set(true);
                self.state.user_scale.set(1.0);
            } else {
                self.state.fit_mode.set(false);
                self.state
                    .user_scale
                    .set((old_scale / fit_scale).clamp(zoom::MIN_ZOOM, zoom::MAX_ZOOM));
            }
        };
        self.apply_scale();
    }

    fn connect_input(self: &Rc<Self>) {
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let viewer = Rc::downgrade(self);
            move |_, x, y| {
                if let Some(viewer) = viewer.upgrade() {
                    viewer.state.pointer_position.replace(Some((x, y)));
                }
            }
        });
        motion.connect_motion({
            let viewer = Rc::downgrade(self);
            move |_, x, y| {
                if let Some(viewer) = viewer.upgrade() {
                    viewer.state.pointer_position.replace(Some((x, y)));
                }
            }
        });
        motion.connect_leave({
            let viewer = Rc::downgrade(self);
            move |_| {
                if let Some(viewer) = viewer.upgrade() {
                    viewer.state.pointer_position.replace(None);
                }
            }
        });
        self.scroller.add_controller(motion);

        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.connect_scroll({
            let viewer = Rc::downgrade(self);
            move |_, _, dy| {
                let Some(viewer) = viewer.upgrade() else {
                    return gtk::glib::Propagation::Stop;
                };
                if !viewer.has_loaded_image() || dy.abs() <= f64::EPSILON {
                    return gtk::glib::Propagation::Proceed;
                }

                let current = viewer.current_scale();
                let scale = if dy > 0.0 {
                    current / zoom::ZOOM_STEP
                } else {
                    current * zoom::ZOOM_STEP
                };
                let pointer = viewer.state.pointer_position.borrow().as_ref().copied();
                viewer.apply_zoom_with_pointer(scale, pointer);
                gtk::glib::Propagation::Stop
            }
        });
        self.scroller.add_controller(wheel);

        let dbl = gtk::GestureClick::builder().button(1).build();
        dbl.connect_pressed({
            let viewer = Rc::downgrade(self);
            move |gesture, n_press, x, y| {
                let Some(viewer) = viewer.upgrade() else {
                    return;
                };
                if n_press != 2 {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }

                if !viewer.has_loaded_image() {
                    return;
                }

                gesture.set_state(gtk::EventSequenceState::Claimed);
                if viewer.state.fit_mode.get() {
                    viewer.apply_zoom_with_pointer(
                        viewer.state.fit_scale.get() * zoom::ZOOM_STEP * zoom::ZOOM_STEP,
                        Some((x, y)),
                    );
                } else {
                    viewer.state.fit_mode.set(true);
                    viewer.state.user_scale.set(1.0);
                    viewer.apply_scale();
                }
            }
        });
        self.scroller.add_controller(dbl);

        let drag = gtk::GestureDrag::builder().button(0).build();
        drag.connect_drag_begin({
            let viewer = Rc::downgrade(self);
            move |gesture, _, _| {
                let Some(viewer) = viewer.upgrade() else {
                    return;
                };

                if !viewer.has_loaded_image() {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }

                let hadj = viewer.scroller.hadjustment();
                let vadj = viewer.scroller.vadjustment();
                let can_drag_x = (hadj.upper() - hadj.lower() - hadj.page_size()) > 0.0;
                let can_drag_y = (vadj.upper() - vadj.lower() - vadj.page_size()) > 0.0;
                if !can_drag_x && !can_drag_y {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }

                viewer.state.drag_origin.replace(Some((0.0, 0.0)));
                let cursor = gdk::Cursor::from_name("grabbing", None);
                if let Some(cursor) = cursor {
                    viewer.scroller.set_cursor(Some(&cursor));
                }
            }
        });
        drag.connect_drag_update({
            let viewer = Rc::downgrade(self);
            move |_, dx, dy| {
                let Some(viewer) = viewer.upgrade() else {
                    return;
                };
                let Some((start_x, start_y)) = viewer.state.drag_origin.borrow().as_ref().copied()
                else {
                    return;
                };

                let hadj = viewer.scroller.hadjustment();
                let vadj = viewer.scroller.vadjustment();
                hadj.set_value(
                    (hadj.value() - (dx - start_x))
                        .clamp(0.0, (hadj.upper() - hadj.page_size()).max(0.0)),
                );
                vadj.set_value(
                    (vadj.value() - (dy - start_y))
                        .clamp(0.0, (vadj.upper() - vadj.page_size()).max(0.0)),
                );

                viewer.state.drag_origin.replace(Some((dx, dy)));
            }
        });
        drag.connect_drag_end({
            let viewer = Rc::downgrade(self);
            move |_, _, _| {
                if let Some(viewer) = viewer.upgrade() {
                    viewer.scroller.set_cursor(None);
                    viewer.state.drag_origin.replace(None);
                }
            }
        });
        self.scroller.add_controller(drag);

        self.scroller.hadjustment().connect_page_size_notify({
            let viewer = Rc::downgrade(self);
            move |_| {
                if let Some(viewer) = viewer.upgrade() {
                    viewer.recalculate_fit();
                }
            }
        });
        self.scroller.vadjustment().connect_page_size_notify({
            let viewer = Rc::downgrade(self);
            move |_| {
                if let Some(viewer) = viewer.upgrade() {
                    viewer.recalculate_fit();
                }
            }
        });
    }

    fn show_error(&self, message: &str) {
        self.picture.set_paintable(Option::<&gdk::Paintable>::None);
        self.picture.set_visible(false);
        self.picture.set_size_request(-1, -1);
        self.state.fit_scale.set(1.0);
        self.state.user_scale.set(1.0);
        self.state.fit_mode.set(true);
        self.state.source_size.replace(None);
        self.status.set_text(message);
        self.status.set_visible(true);
        self.state.pointer_position.replace(None);
        self.state.drag_origin.replace(None);
    }
}

fn scale_dimension(source_dimension: i32, scale: f64) -> i32 {
    let scaled = (f64::from(source_dimension) * scale).max(0.0).round();
    match scaled.is_finite() {
        true => (scaled.min(f64::from(i32::MAX))).max(1.0) as i32,
        false => 1,
    }
}
