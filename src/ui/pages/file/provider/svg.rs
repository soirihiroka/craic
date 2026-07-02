use super::{PreviewMatchRequest, PreviewRequest};
use crate::git;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const ZOOM_IN_STEP: f64 = 1.1;
const ZOOM_OUT_STEP: f64 = 0.9;
const MIN_ZOOM: f64 = 0.125;
const MAX_ZOOM: f64 = 16.0;

struct SvgPreviewLoad {
    bytes: Vec<u8>,
    text: String,
    signature: super::ContentSignature,
    comparison: Option<git::FileComparison>,
    markdown_lint_issues: Vec<crate::markdown_lint::MarkdownLintIssue>,
    spellcheck_issues: Vec<crate::spellcheck::SpellcheckIssue>,
}

pub(in crate::ui::pages::file) struct SvgPreview {
    pub(in crate::ui::pages::file) root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    picture: gtk::Picture,
    empty: gtk::Label,
    signature: RefCell<Option<super::ContentSignature>>,
    zoom_factor: Cell<f64>,
    svg_bytes: RefCell<Option<Vec<u8>>>,
    source_size: RefCell<Option<(i32, i32)>>,
    texture: RefCell<Option<gdk::Texture>>,
    pointer_position: RefCell<Option<(f64, f64)>>,
}

impl SvgPreview {
    pub(in crate::ui::pages::file) fn new() -> Rc<Self> {
        let checkerboard = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        checkerboard.set_sensitive(false);
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
            .can_shrink(false)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        picture.set_visible(false);

        let empty = widgets::muted("No image");
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);

        let canvas = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .build();
        canvas.append(&picture);
        canvas.append(&empty);

        let scroller = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&canvas)
            .build();
        scroller.set_has_frame(false);
        scroller.add_css_class("svg-preview-scroller");

        let overlay = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        overlay.set_child(Some(&checkerboard));
        overlay.add_overlay(&scroller);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&overlay);

        let preview = Rc::new(Self {
            root,
            scroller,
            picture,
            empty,
            signature: RefCell::new(None),
            zoom_factor: Cell::new(1.0),
            svg_bytes: RefCell::new(None),
            source_size: RefCell::new(None),
            texture: RefCell::new(None),
            pointer_position: RefCell::new(None),
        });

        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        wheel.connect_scroll({
            let preview = Rc::downgrade(&preview);
            move |_, _, dy| {
                if dy.abs() <= f64::EPSILON {
                    return gtk::glib::Propagation::Proceed;
                }

                let Some(preview) = preview.upgrade() else {
                    return gtk::glib::Propagation::Stop;
                };
                let next = if dy > 0.0 {
                    ZOOM_OUT_STEP
                } else {
                    ZOOM_IN_STEP
                } * preview.zoom_factor.get();
                preview.set_zoom_factor_at_pointer(next);
                gtk::glib::Propagation::Stop
            }
        });
        preview.scroller.add_controller(wheel);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let preview = Rc::downgrade(&preview);
            move |_, x, y| {
                if let Some(preview) = preview.upgrade() {
                    preview.pointer_position.replace(Some((x, y)));
                }
            }
        });
        motion.connect_motion({
            let preview = Rc::downgrade(&preview);
            move |_, x, y| {
                if let Some(preview) = preview.upgrade() {
                    preview.pointer_position.replace(Some((x, y)));
                }
            }
        });
        motion.connect_leave({
            let preview = Rc::downgrade(&preview);
            move |_| {
                if let Some(preview) = preview.upgrade() {
                    preview.pointer_position.replace(None);
                }
            }
        });
        preview.scroller.add_controller(motion);

        preview.scroller.hadjustment().connect_page_size_notify({
            let preview = Rc::downgrade(&preview);
            move |_| {
                if let Some(preview) = preview.upgrade()
                    && preview.svg_bytes.borrow().is_some()
                {
                    preview.apply_zoom(preview.zoom_factor.get());
                }
            }
        });
        preview.scroller.vadjustment().connect_page_size_notify({
            let preview = Rc::downgrade(&preview);
            move |_| {
                if let Some(preview) = preview.upgrade()
                    && preview.svg_bytes.borrow().is_some()
                {
                    preview.apply_zoom(preview.zoom_factor.get());
                }
            }
        });

        preview
    }

    pub(in crate::ui::pages::file) fn set_svg(
        &self,
        bytes: &[u8],
        signature: super::ContentSignature,
    ) {
        if self.signature.borrow().as_ref() == Some(&signature) {
            return;
        }

        let Some((width, height)) = svg_intrinsic_size(bytes) else {
            self.show_unavailable("Unable to load image");
            self.signature.replace(Some(signature));
            return;
        };

        let keep_zoom = match self.source_size.borrow().as_ref().copied() {
            Some((w, h)) if w == width && h == height => Some(self.zoom_factor.get()),
            _ => None,
        };

        self.signature.replace(Some(signature));
        self.source_size.replace(Some((width, height)));
        self.svg_bytes.replace(Some(bytes.to_vec()));
        self.empty.set_visible(false);

        match keep_zoom {
            Some(zoom) => self.apply_zoom(zoom),
            None => {
                self.zoom_factor.set(1.0);
                self.apply_zoom(1.0);
            }
        }
    }

    fn show_unavailable(&self, message: &str) {
        self.empty.set_label(message);
        self.empty.set_visible(true);
        self.picture.set_visible(false);
        self.picture.set_paintable(Option::<&gdk::Paintable>::None);
        self.texture.replace(None);
        self.source_size.replace(None);
        self.svg_bytes.replace(None);
    }

    fn set_zoom_factor_at_pointer(&self, factor: f64) {
        let Some((source_width, source_height)) = *self.source_size.borrow() else {
            self.apply_zoom(factor);
            return;
        };
        let Some((pointer_x, pointer_y)) = self.pointer_position.borrow().as_ref().copied() else {
            self.apply_zoom(factor);
            return;
        };

        let viewport_width = self.scroller.allocated_width() as f64;
        let viewport_height = self.scroller.allocated_height() as f64;
        let fit_scale = self.current_fit_scale(source_width, source_height);
        let old_scale = (fit_scale * self.zoom_factor.get()).max(0.01);
        let old_render_width = (source_width as f64 * old_scale).max(1.0);
        let old_render_height = (source_height as f64 * old_scale).max(1.0);
        let old_origin_x = ((viewport_width - old_render_width) / 2.0).max(0.0);
        let old_origin_y = ((viewport_height - old_render_height) / 2.0).max(0.0);
        let hadj = self.scroller.hadjustment();
        let vadj = self.scroller.vadjustment();

        let content_x = hadj.value() + pointer_x - old_origin_x;
        let content_y = vadj.value() + pointer_y - old_origin_y;
        let ratio_x = (content_x / old_render_width).clamp(0.0, 1.0);
        let ratio_y = (content_y / old_render_height).clamp(0.0, 1.0);

        let zoom = factor.clamp(MIN_ZOOM, MAX_ZOOM);
        let new_scale = (fit_scale * zoom).max(0.01);
        let new_render_width = (source_width as f64 * new_scale).max(1.0);
        let new_render_height = (source_height as f64 * new_scale).max(1.0);
        let new_origin_x = ((viewport_width - new_render_width) / 2.0).max(0.0);
        let new_origin_y = ((viewport_height - new_render_height) / 2.0).max(0.0);

        self.apply_zoom(zoom);

        let max_hscroll = (new_render_width - viewport_width).max(0.0);
        let max_vscroll = (new_render_height - viewport_height).max(0.0);
        self.scroller.hadjustment().set_value(
            (ratio_x * new_render_width + new_origin_x - pointer_x).clamp(0.0, max_hscroll),
        );
        self.scroller.vadjustment().set_value(
            (ratio_y * new_render_height + new_origin_y - pointer_y).clamp(0.0, max_vscroll),
        );
    }

    fn current_fit_scale(&self, source_width: i32, source_height: i32) -> f64 {
        let viewport_width = self.scroller.allocated_width();
        let viewport_height = self.scroller.allocated_height();
        if viewport_width <= 0 || viewport_height <= 0 {
            return 1.0;
        }

        (viewport_width as f64 / source_width as f64)
            .min(viewport_height as f64 / source_height as f64)
            .max(0.0)
    }

    fn apply_zoom(&self, factor: f64) {
        let svg_bytes = self.svg_bytes.borrow();
        let Some(bytes) = svg_bytes.as_deref() else {
            return;
        };
        let Some((source_width, source_height)) = *self.source_size.borrow() else {
            return;
        };

        let zoom = factor.clamp(MIN_ZOOM, MAX_ZOOM);
        self.zoom_factor.set(zoom);
        let scale = (self.current_fit_scale(source_width, source_height) * zoom).max(0.01);
        let logical_width = (source_width as f64 * scale)
            .max(1.0)
            .round()
            .min(i32::MAX as f64) as i32;
        let logical_height = (source_height as f64 * scale)
            .max(1.0)
            .round()
            .min(i32::MAX as f64) as i32;

        self.picture.set_size_request(logical_width, logical_height);
        let device_scale = self.picture.scale_factor() as f64;
        let physical_width = (logical_width as f64 * device_scale)
            .max(1.0)
            .round()
            .min(i32::MAX as f64) as i32;
        let physical_height = (logical_height as f64 * device_scale)
            .max(1.0)
            .round()
            .min(i32::MAX as f64) as i32;

        let Some(texture) = load_texture(bytes, physical_width, physical_height) else {
            self.show_unavailable("Unable to load image");
            return;
        };

        self.texture.replace(Some(texture.clone()));
        self.picture.set_paintable(Some(&texture));
        self.picture.set_visible(true);
        self.empty.set_visible(false);
    }
}

pub(in crate::ui::pages::file) fn show(request: PreviewRequest<'_>) {
    show_svg(request, None);
}

pub(in crate::ui::pages::file) fn show_match(request: PreviewMatchRequest<'_>) {
    let selection = Some((request.start, request.end));
    show_svg(request.into_preview_request(), selection);
}

fn show_svg(request: PreviewRequest<'_>, selection: Option<(usize, usize)>) {
    request.right.show_editor_loading(request.file_path, "SVG");

    let files = request.files.clone();
    let file_path = request.file_path.to_string();
    let read_node_path = request.node_path.clone();
    let apply_node_path = request.node_path.clone();
    let git = (request.ctx.system_ref().provider_kind == crate::system::ProviderKind::Local)
        .then(|| request.ctx.git())
        .flatten();
    let prefetched_bytes = request.prefetched_bytes.map(|bytes| bytes.to_vec());
    let apply_file_path = file_path.clone();
    let disk_signature = super::disk_signature(request.info);
    let writable = request.info.capabilities.writable;
    let language = crate::ui::content::code_editor::language_hint_from_path(&file_path);

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || {
            read_svg_source(prefetched_bytes, files.as_ref(), &read_node_path).map(
                |(bytes, text, signature)| {
                    let comparison = git.as_ref().and_then(|git| git.comparison(&file_path).ok());
                    let allowlist = crate::spellcheck::SpellcheckAllowlist::default();
                    let spellcheck_issues = crate::spellcheck::check_document(
                        &language,
                        Some(&file_path),
                        &text,
                        &allowlist,
                    );
                    SvgPreviewLoad {
                        bytes,
                        text,
                        signature,
                        comparison,
                        markdown_lint_issues: Vec::new(),
                        spellcheck_issues,
                    }
                },
            )
        },
        move |right, result| match result {
            Ok(load) => {
                right.show_editor(
                    &apply_node_path,
                    &apply_file_path,
                    &load.text,
                    disk_signature,
                    writable,
                    load.comparison.as_ref(),
                    load.markdown_lint_issues,
                    load.spellcheck_issues,
                );
                right.file_svg_preview.set_svg(&load.bytes, load.signature);
                right
                    .file_view_split
                    .set_end_child(Some(&right.file_svg_preview.root));
                if let Some((start, end)) = selection {
                    right.file_editor.select_range(start, end);
                }
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

fn read_svg_source(
    prefetched_bytes: Option<Vec<u8>>,
    files: &dyn crate::system::capabilities::files::FileAccess,
    node_path: &crate::system::FileNodePath,
) -> Result<(Vec<u8>, String, super::ContentSignature), String> {
    let bytes =
        super::super::read_repository_file_bytes_from_prefetch(prefetched_bytes, files, node_path)?;
    let signature = super::content_signature(&bytes);
    let text = String::from_utf8(bytes.clone())
        .map_err(|_| "Unable to load SVG source. File is not valid UTF-8 text.".to_string())?;

    Ok((bytes, text, signature))
}

fn svg_intrinsic_size(bytes: &[u8]) -> Option<(i32, i32)> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    if loader.write(bytes).is_err() || loader.close().is_err() {
        return None;
    }

    let pixbuf = loader.pixbuf()?;
    let width = pixbuf.width();
    let height = pixbuf.height();
    (width > 0 && height > 0).then_some((width, height))
}

fn load_texture(bytes: &[u8], width: i32, height: i32) -> Option<gdk::Texture> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.set_size(width, height);
    if loader.write(bytes).is_err() || loader.close().is_err() {
        return None;
    }

    loader
        .pixbuf()
        .map(|pixbuf| gdk::Texture::for_pixbuf(&pixbuf))
}
