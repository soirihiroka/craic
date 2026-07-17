use crate::ui::{canvas_scroll, components::context_menu, widgets};
use adw::prelude::*;
use gtk::{cairo, gdk};
use poppler::{Document, Page, Rectangle, SelectionStyle};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Sender, TryRecvError},
};
use std::thread;
use std::time::{Duration, Instant};

const PDF_BASE_MAX_WIDTH: f64 = 900.0;
const PDF_MIN_ZOOM: f64 = 0.5;
const PDF_MAX_ZOOM: f64 = 3.0;
const PDF_ZOOM_STEP: f64 = 1.25;
const PDF_ZOOM_RERENDER_DELAY_MS: u64 = 140;
const PDF_RENDER_POLL_INTERVAL_MS: u64 = 16;
const PDF_RENDER_RESULT_BATCH_LIMIT: usize = 3;
const PDF_RENDER_WORKER_LIMIT: usize = 4;
const PDF_RENDER_CACHE_MAX_BYTES: usize = 192 * 1024 * 1024;
const PDF_RENDER_CACHE_MAX_PAGE_BYTES: usize = 64 * 1024 * 1024;
const PDF_PAGE_GAP: i32 = 22;
const PDF_PAGE_MARGIN: i32 = 18;
const PDF_PAGE_SHADOW_MARGIN: i32 = 8;
const PDF_PAGE_VERTICAL_GAP: i32 = (PDF_PAGE_GAP + (PDF_PAGE_SHADOW_MARGIN * 2)) / 2;
const PDF_SELECTION_MIN_DISTANCE: f64 = 1.0;

pub struct PdfPreview {
    pub root: gtk::Box,
    viewer: gtk::Overlay,
    scroller: gtk::ScrolledWindow,
    pages: gtk::Box,
    empty: gtk::Label,
    generation: Rc<Cell<u64>>,
    identity: Rc<RefCell<Option<PdfPreviewIdentity>>>,
    zoom: Rc<Cell<f64>>,
    document: Rc<RefCell<Option<Document>>>,
    bytes: Rc<RefCell<Option<Arc<Vec<u8>>>>>,
    page_count: Rc<Cell<i32>>,
    page_widgets: Rc<RefCell<Vec<PdfPageWidget>>>,
    selection: Rc<RefCell<Option<PdfTextSelection>>>,
    render_cache: Rc<RefCell<PdfRenderCache>>,
    render_source: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    rerender_source: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    autoscroll: Rc<canvas_scroll::MiddleAutoscroll>,
    autoscroll_marker: gtk::DrawingArea,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PdfPreviewIdentity {
    len: usize,
    hash: u64,
}

#[derive(Clone)]
struct PdfByteStore(Arc<Vec<u8>>);

impl AsRef<[u8]> for PdfByteStore {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PdfZoomAction {
    In,
    Out,
}

#[derive(Clone)]
struct PdfPageWidget {
    page_index: i32,
    picture: gtk::Picture,
    overlay: gtk::Overlay,
    selection_area: gtk::DrawingArea,
    page_width: f64,
    page_height: f64,
    base_scale: f64,
}

#[derive(Clone, Debug)]
struct PdfTextSelection {
    kind: PdfTextSelectionKind,
    text: String,
}

#[derive(Clone, Debug)]
enum PdfTextSelectionKind {
    Range {
        start: PdfSelectionPoint,
        end: PdfSelectionPoint,
    },
    AllPages,
}

#[derive(Clone, Copy, Debug)]
struct PdfSelectionPoint {
    page_index: i32,
    point: PdfPoint,
}

#[derive(Clone, Copy, Debug)]
struct PdfPoint {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PdfRenderCacheKey {
    identity: PdfPreviewIdentity,
    page_index: i32,
    zoom_key: u32,
}

#[derive(Clone)]
struct RenderedPdfPage {
    texture: gdk::MemoryTexture,
    page_index: i32,
    byte_size: usize,
}

struct RenderedPdfPagePixels {
    page_index: i32,
    width: i32,
    height: i32,
    stride: usize,
    pixels: Vec<u8>,
}

struct PdfPageRenderError {
    page_index: i32,
    error: String,
}

enum PdfRenderResult {
    Page(Result<RenderedPdfPagePixels, PdfPageRenderError>),
    WorkerError(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PdfContextAction {
    Copy,
    SelectAll,
}

#[derive(Default)]
struct PdfRenderCache {
    entries: HashMap<PdfRenderCacheKey, RenderedPdfPage>,
    order: VecDeque<PdfRenderCacheKey>,
    byte_size: usize,
}

impl PdfRenderCache {
    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.byte_size = 0;
    }

    fn get(&mut self, key: PdfRenderCacheKey) -> Option<RenderedPdfPage> {
        let page = self.entries.get(&key).cloned()?;
        self.touch(key);
        Some(page)
    }

    fn insert(&mut self, key: PdfRenderCacheKey, page: RenderedPdfPage) {
        if page.byte_size > PDF_RENDER_CACHE_MAX_PAGE_BYTES {
            return;
        }

        if let Some(previous) = self.entries.remove(&key) {
            self.byte_size = self.byte_size.saturating_sub(previous.byte_size);
            self.remove_order_key(key);
        }

        self.byte_size = self.byte_size.saturating_add(page.byte_size);
        self.entries.insert(key, page);
        self.order.push_back(key);
        self.trim();
    }

    fn touch(&mut self, key: PdfRenderCacheKey) {
        self.remove_order_key(key);
        self.order.push_back(key);
    }

    fn remove_order_key(&mut self, key: PdfRenderCacheKey) {
        if let Some(index) = self.order.iter().position(|candidate| *candidate == key) {
            self.order.remove(index);
        }
    }

    fn trim(&mut self) {
        while self.byte_size > PDF_RENDER_CACHE_MAX_BYTES {
            let Some(key) = self.order.pop_front() else {
                self.byte_size = 0;
                break;
            };
            if let Some(page) = self.entries.remove(&key) {
                self.byte_size = self.byte_size.saturating_sub(page.byte_size);
            }
        }
    }
}

impl PdfPreview {
    pub fn new() -> Self {
        let generation = Rc::new(Cell::new(0));
        let identity = Rc::new(RefCell::new(None));
        let zoom = Rc::new(Cell::new(1.0));
        let document: Rc<RefCell<Option<Document>>> = Rc::new(RefCell::new(None));
        let bytes: Rc<RefCell<Option<Arc<Vec<u8>>>>> = Rc::new(RefCell::new(None));
        let page_count = Rc::new(Cell::new(0));
        let page_widgets = Rc::new(RefCell::new(Vec::new()));
        let selection = Rc::new(RefCell::new(None));
        let render_cache = Rc::new(RefCell::new(PdfRenderCache::default()));
        let render_source = Rc::new(RefCell::new(None));
        let rerender_source = Rc::new(RefCell::new(None));
        let autoscroll = Rc::new(canvas_scroll::MiddleAutoscroll::new());
        let pointer_position = Rc::new(Cell::new(None::<(f64, f64)>));

        let pages = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(PDF_PAGE_VERTICAL_GAP)
            .halign(gtk::Align::Center)
            .margin_top(PDF_PAGE_VERTICAL_GAP)
            .margin_bottom(PDF_PAGE_VERTICAL_GAP)
            .margin_start(PDF_PAGE_MARGIN)
            .margin_end(PDF_PAGE_MARGIN)
            .build();
        pages.add_css_class("pdf-preview-pages");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&pages)
            .build();
        scroller.add_css_class("pdf-preview-scroller");
        scroller.set_focusable(true);

        let autoscroll_marker = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .can_target(false)
            .build();
        canvas_scroll::install_scrolled_window_middle_autoscroll_with_state(
            &scroller,
            &autoscroll_marker,
            &autoscroll,
            canvas_scroll::AutoscrollAxes::Both,
            "pdf_preview",
            {
                let scroller = scroller.clone();
                let page_widgets = Rc::clone(&page_widgets);
                move |cursor| set_pdf_autoscroll_cursor(&scroller, &page_widgets, cursor)
            },
        );

        let viewer = gtk::Overlay::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&scroller)
            .build();
        viewer.add_overlay(&autoscroll_marker);
        viewer.set_visible(false);

        let empty = widgets::muted("No PDF");
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);
        empty.set_hexpand(true);
        empty.set_vexpand(true);
        empty.set_visible(false);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&viewer);
        root.append(&empty);
        root.set_visible(false);

        install_pdf_pointer_tracking(&scroller, &pointer_position);
        install_pdf_context_menu(&scroller, &document, &selection, &page_widgets);
        install_pdf_zoom_scroll(
            &scroller,
            &zoom,
            &document,
            &bytes,
            &identity,
            &pages,
            &generation,
            &page_count,
            &page_widgets,
            &selection,
            &render_cache,
            &render_source,
            &rerender_source,
            &pointer_position,
        );
        install_pdf_key_shortcuts(
            &scroller,
            &zoom,
            &document,
            &bytes,
            &identity,
            &pages,
            &generation,
            &page_count,
            &page_widgets,
            &selection,
            &render_cache,
            &render_source,
            &rerender_source,
            &pointer_position,
        );

        Self {
            root,
            viewer,
            scroller,
            pages,
            empty,
            generation,
            identity,
            zoom,
            document,
            bytes,
            page_count,
            page_widgets,
            selection,
            render_cache,
            render_source,
            rerender_source,
            autoscroll,
            autoscroll_marker,
        }
    }

    pub fn set_pdf(&self, file_path: &str, bytes: &[u8]) {
        let identity = PdfPreviewIdentity::for_bytes(bytes);
        self.root.set_visible(true);
        if self.identity.borrow().as_ref() == Some(&identity) {
            log::debug!(
                "PDF preview request reused path={file_path} bytes={} hash={} generation={}",
                identity.len,
                identity.hash,
                self.generation.get()
            );
            return;
        }

        self.clear_content();
        self.identity.replace(Some(identity));
        self.zoom.set(1.0);
        let bytes = Arc::new(bytes.to_vec());
        self.bytes.replace(Some(Arc::clone(&bytes)));
        let generation = self.generation.get();
        log::info!(
            "PDF preview scheduled path={file_path} bytes={} generation={generation}",
            bytes.len()
        );

        let started = Instant::now();
        let document_bytes = gtk::glib::Bytes::from_owned(PdfByteStore(Arc::clone(&bytes)));
        let document = match Document::from_bytes(&document_bytes, None) {
            Ok(document) => document,
            Err(err) => {
                self.show_message("Unable to load PDF");
                log::warn!("Failed to load PDF preview for {file_path}: {err}");
                return;
            }
        };
        let page_count = document.n_pages();
        log::info!(
            "PDF preview loaded with Poppler path={file_path} pages={page_count} elapsed_ms={}",
            started.elapsed().as_millis()
        );

        if page_count <= 0 {
            self.show_message("PDF has no pages");
            return;
        }

        self.document.replace(Some(document.clone()));
        self.page_count.set(page_count);
        self.empty.set_visible(false);
        self.viewer.set_visible(true);
        build_pdf_page_placeholders(
            &document,
            &self.pages,
            &self.scroller,
            Rc::clone(&self.page_widgets),
            Rc::clone(&self.selection),
            Rc::clone(&self.zoom),
            Rc::clone(&self.autoscroll),
        );
        schedule_poppler_render(
            bytes,
            identity,
            page_indices_for_count(page_count),
            Rc::clone(&self.generation),
            generation,
            self.zoom.get(),
            file_path.to_string(),
            Rc::clone(&self.page_widgets),
            Rc::clone(&self.zoom),
            Rc::clone(&self.render_cache),
            Rc::clone(&self.render_source),
        );
    }

    pub fn clear(&self) {
        self.clear_content();
        self.identity.borrow_mut().take();
        self.root.set_visible(false);
    }

    fn clear_content(&self) {
        stop_pdf_middle_autoscroll(
            &self.scroller,
            &self.page_widgets,
            &self.autoscroll,
            &self.autoscroll_marker,
        );
        cancel_pdf_rerender(&self.rerender_source);
        cancel_pdf_render_receiver(&self.render_source);
        self.generation.set(self.generation.get().wrapping_add(1));
        self.document.borrow_mut().take();
        self.bytes.borrow_mut().take();
        self.page_count.set(0);
        self.zoom.set(1.0);
        self.selection.borrow_mut().take();
        self.render_cache.borrow_mut().clear();
        self.page_widgets.borrow_mut().clear();
        clear_pdf_pages(&self.pages);
        self.viewer.set_visible(false);
        self.empty.set_visible(false);
    }

    fn show_message(&self, message: &str) {
        stop_pdf_middle_autoscroll(
            &self.scroller,
            &self.page_widgets,
            &self.autoscroll,
            &self.autoscroll_marker,
        );
        self.root.set_visible(true);
        self.viewer.set_visible(false);
        self.empty.set_label(message);
        self.empty.set_visible(true);
    }
}

fn install_pdf_pointer_tracking(
    scroller: &gtk::ScrolledWindow,
    pointer_position: &Rc<Cell<Option<(f64, f64)>>>,
) {
    let motion = gtk::EventControllerMotion::new();
    let pointer_position_for_enter = Rc::clone(pointer_position);
    motion.connect_enter(move |_, x, y| pointer_position_for_enter.set(Some((x, y))));
    let pointer_position_for_motion = Rc::clone(pointer_position);
    motion.connect_motion(move |_, x, y| pointer_position_for_motion.set(Some((x, y))));
    let pointer_position_for_leave = Rc::clone(pointer_position);
    motion.connect_leave(move |_| pointer_position_for_leave.set(None));
    scroller.add_controller(motion);
}

fn install_pdf_context_menu(
    scroller: &gtk::ScrolledWindow,
    document: &Rc<RefCell<Option<Document>>>,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
) {
    let click = gtk::GestureClick::builder().button(0).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let scroller = scroller.clone();
        let document = Rc::clone(document);
        let selection = Rc::clone(selection);
        let page_widgets = Rc::clone(page_widgets);

        move |gesture, _, x, y| {
            if gesture.current_button() != 3 {
                return;
            }

            scroller.grab_focus();
            show_pdf_context_menu(&scroller, &document, &selection, &page_widgets, x, y);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    scroller.add_controller(click);
}

fn show_pdf_context_menu(
    scroller: &gtk::ScrolledWindow,
    document: &Rc<RefCell<Option<Document>>>,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    x: f64,
    y: f64,
) {
    context_menu::popup_action_menu(
        scroller,
        x,
        y,
        pdf_context_menu_sections(document, selection),
        {
            let scroller = scroller.clone();
            let document = Rc::clone(document);
            let selection = Rc::clone(selection);
            let page_widgets = Rc::clone(page_widgets);

            move |action| {
                run_pdf_context_action(&document, &selection, &page_widgets, action);
                scroller.grab_focus();
            }
        },
    );
}

fn pdf_context_menu_sections(
    document: &Rc<RefCell<Option<Document>>>,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
) -> Vec<context_menu::ActionMenuSection<PdfContextAction>> {
    vec![context_menu::ActionMenuSection::new(vec![
        context_menu::ActionMenuItem::new(
            "Copy",
            PdfContextAction::Copy,
            pdf_selection_clipboard_text(selection).is_some(),
        ),
        context_menu::ActionMenuItem::new(
            "Select All",
            PdfContextAction::SelectAll,
            document
                .borrow()
                .as_ref()
                .is_some_and(|document| document.n_pages() > 0),
        ),
    ])]
}

fn run_pdf_context_action(
    document: &Rc<RefCell<Option<Document>>>,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    action: PdfContextAction,
) {
    match action {
        PdfContextAction::Copy => {
            copy_pdf_selection(selection);
        }
        PdfContextAction::SelectAll => {
            select_all_pdf_text(document, selection, page_widgets);
        }
    }
}

fn stop_pdf_middle_autoscroll(
    scroller: &gtk::ScrolledWindow,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    autoscroll: &Rc<canvas_scroll::MiddleAutoscroll>,
    marker: &gtk::DrawingArea,
) {
    if autoscroll.stop() {
        set_pdf_autoscroll_cursor(scroller, page_widgets, None);
        marker.queue_draw();
    }
}

fn set_pdf_autoscroll_cursor(
    scroller: &gtk::ScrolledWindow,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    cursor: Option<&'static str>,
) {
    scroller.set_cursor_from_name(cursor);
    for widget in page_widgets.borrow().iter() {
        widget.selection_area.set_cursor_from_name(cursor);
    }
}

impl PdfPreviewIdentity {
    fn for_bytes(bytes: &[u8]) -> Self {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        Self {
            len: bytes.len(),
            hash: hasher.finish(),
        }
    }
}

impl PdfPageWidget {
    fn set_display_zoom(&self, zoom: f64) {
        let scale = (self.base_scale * zoom).max(0.01);
        let width = (self.page_width * scale).ceil().max(1.0) as i32;
        let height = (self.page_height * scale).ceil().max(1.0) as i32;
        self.picture.set_size_request(width, height);
        self.overlay.set_size_request(width, height);
        self.selection_area.set_content_width(width);
        self.selection_area.set_content_height(height);
        self.selection_area.set_size_request(width, height);
        self.selection_area.queue_draw();
    }
}

fn install_pdf_zoom_scroll(
    scroller: &gtk::ScrolledWindow,
    pdf_zoom: &Rc<Cell<f64>>,
    pdf_document: &Rc<RefCell<Option<Document>>>,
    pdf_bytes: &Rc<RefCell<Option<Arc<Vec<u8>>>>>,
    pdf_identity: &Rc<RefCell<Option<PdfPreviewIdentity>>>,
    pdf_pages: &gtk::Box,
    pdf_generation: &Rc<Cell<u64>>,
    pdf_page_count: &Rc<Cell<i32>>,
    pdf_page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    pdf_selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    pdf_render_cache: &Rc<RefCell<PdfRenderCache>>,
    pdf_render_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
    pdf_rerender_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
    pointer_position: &Rc<Cell<Option<(f64, f64)>>>,
) {
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scroller_for_scroll = scroller.clone();
    let pdf_zoom = Rc::clone(pdf_zoom);
    let pdf_document = Rc::clone(pdf_document);
    let pdf_bytes = Rc::clone(pdf_bytes);
    let pdf_identity = Rc::clone(pdf_identity);
    let pdf_pages = pdf_pages.clone();
    let pdf_generation = Rc::clone(pdf_generation);
    let pdf_page_count = Rc::clone(pdf_page_count);
    let pdf_page_widgets = Rc::clone(pdf_page_widgets);
    let pdf_selection = Rc::clone(pdf_selection);
    let pdf_render_cache = Rc::clone(pdf_render_cache);
    let pdf_render_source = Rc::clone(pdf_render_source);
    let pdf_rerender_source = Rc::clone(pdf_rerender_source);
    let pointer_position = Rc::clone(pointer_position);

    wheel.connect_scroll(move |controller, dx, dy| {
        let modifiers = controller.current_event_state();
        if !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
            || modifiers.contains(gdk::ModifierType::ALT_MASK)
        {
            return gtk::glib::Propagation::Proceed;
        }

        let action = if dy.abs() >= dx.abs() {
            if dy < 0.0 {
                PdfZoomAction::In
            } else {
                PdfZoomAction::Out
            }
        } else if dx < 0.0 {
            PdfZoomAction::In
        } else {
            PdfZoomAction::Out
        };

        let anchor = controller
            .current_event()
            .and_then(|event| event.position())
            .or_else(|| pointer_position.get())
            .unwrap_or_else(|| viewport_center(&scroller_for_scroll));
        pointer_position.set(Some(anchor));

        if change_pdf_zoom(
            &scroller_for_scroll,
            action,
            anchor,
            &pdf_zoom,
            &pdf_document,
            &pdf_bytes,
            &pdf_identity,
            &pdf_pages,
            &pdf_generation,
            &pdf_page_count,
            &pdf_page_widgets,
            &pdf_selection,
            &pdf_render_cache,
            &pdf_render_source,
            &pdf_rerender_source,
        ) {
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    scroller.add_controller(wheel);
}

fn install_pdf_key_shortcuts(
    scroller: &gtk::ScrolledWindow,
    pdf_zoom: &Rc<Cell<f64>>,
    pdf_document: &Rc<RefCell<Option<Document>>>,
    pdf_bytes: &Rc<RefCell<Option<Arc<Vec<u8>>>>>,
    pdf_identity: &Rc<RefCell<Option<PdfPreviewIdentity>>>,
    pdf_pages: &gtk::Box,
    pdf_generation: &Rc<Cell<u64>>,
    pdf_page_count: &Rc<Cell<i32>>,
    pdf_page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    pdf_selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    pdf_render_cache: &Rc<RefCell<PdfRenderCache>>,
    pdf_render_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
    pdf_rerender_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
    pointer_position: &Rc<Cell<Option<(f64, f64)>>>,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scroller_for_keys = scroller.clone();
    let pdf_zoom = Rc::clone(pdf_zoom);
    let pdf_document = Rc::clone(pdf_document);
    let pdf_bytes = Rc::clone(pdf_bytes);
    let pdf_identity = Rc::clone(pdf_identity);
    let pdf_pages = pdf_pages.clone();
    let pdf_generation = Rc::clone(pdf_generation);
    let pdf_page_count = Rc::clone(pdf_page_count);
    let pdf_page_widgets = Rc::clone(pdf_page_widgets);
    let pdf_selection = Rc::clone(pdf_selection);
    let pdf_render_cache = Rc::clone(pdf_render_cache);
    let pdf_render_source = Rc::clone(pdf_render_source);
    let pdf_rerender_source = Rc::clone(pdf_rerender_source);
    let pointer_position = Rc::clone(pointer_position);

    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
            || modifiers.contains(gdk::ModifierType::ALT_MASK)
        {
            return gtk::glib::Propagation::Proceed;
        }

        let anchor = pointer_position
            .get()
            .unwrap_or_else(|| viewport_center(&scroller_for_keys));

        if key == gdk::Key::plus || key == gdk::Key::equal || key == gdk::Key::KP_Add {
            return if change_pdf_zoom(
                &scroller_for_keys,
                PdfZoomAction::In,
                anchor,
                &pdf_zoom,
                &pdf_document,
                &pdf_bytes,
                &pdf_identity,
                &pdf_pages,
                &pdf_generation,
                &pdf_page_count,
                &pdf_page_widgets,
                &pdf_selection,
                &pdf_render_cache,
                &pdf_render_source,
                &pdf_rerender_source,
            ) {
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            };
        }

        if key == gdk::Key::minus || key == gdk::Key::underscore || key == gdk::Key::KP_Subtract {
            return if change_pdf_zoom(
                &scroller_for_keys,
                PdfZoomAction::Out,
                anchor,
                &pdf_zoom,
                &pdf_document,
                &pdf_bytes,
                &pdf_identity,
                &pdf_pages,
                &pdf_generation,
                &pdf_page_count,
                &pdf_page_widgets,
                &pdf_selection,
                &pdf_render_cache,
                &pdf_render_source,
                &pdf_rerender_source,
            ) {
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            };
        }

        if matches!(key, gdk::Key::c | gdk::Key::C) && copy_pdf_selection(&pdf_selection) {
            return gtk::glib::Propagation::Stop;
        }

        gtk::glib::Propagation::Proceed
    });
    scroller.add_controller(keys);
}

fn change_pdf_zoom(
    scroller: &gtk::ScrolledWindow,
    action: PdfZoomAction,
    anchor: (f64, f64),
    pdf_zoom: &Rc<Cell<f64>>,
    pdf_document: &Rc<RefCell<Option<Document>>>,
    pdf_bytes: &Rc<RefCell<Option<Arc<Vec<u8>>>>>,
    pdf_identity: &Rc<RefCell<Option<PdfPreviewIdentity>>>,
    _pdf_pages: &gtk::Box,
    pdf_generation: &Rc<Cell<u64>>,
    pdf_page_count: &Rc<Cell<i32>>,
    pdf_page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    _pdf_selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    pdf_render_cache: &Rc<RefCell<PdfRenderCache>>,
    pdf_render_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
    pdf_rerender_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
) -> bool {
    let current = pdf_zoom.get();
    let next = match action {
        PdfZoomAction::In => (current * PDF_ZOOM_STEP).min(PDF_MAX_ZOOM),
        PdfZoomAction::Out => (current / PDF_ZOOM_STEP).max(PDF_MIN_ZOOM),
    };

    if (current - next).abs() <= f64::EPSILON {
        return true;
    }

    if pdf_document.borrow().is_none() {
        return false;
    }
    let Some(bytes) = pdf_bytes.borrow().clone() else {
        return false;
    };
    let Some(identity) = pdf_identity.borrow().as_ref().copied() else {
        return false;
    };
    let page_count = pdf_page_count.get();
    if page_count <= 0 {
        return false;
    }

    pdf_zoom.set(next);
    apply_pdf_zoom_to_widgets(pdf_page_widgets, next);
    keep_pdf_anchor_at_pointer(scroller, current, next, anchor);
    pdf_generation.set(pdf_generation.get().wrapping_add(1));
    let generation = pdf_generation.get();
    log::info!("PDF preview zoom changed zoom={next:.2} generation={generation}");

    cancel_pdf_render_receiver(pdf_render_source);
    let missing_pages = apply_cached_pdf_pages(
        pdf_render_cache,
        identity,
        next,
        page_count,
        pdf_page_widgets,
        pdf_zoom,
    );
    schedule_debounced_poppler_render(
        bytes,
        identity,
        missing_pages,
        pdf_generation,
        generation,
        next,
        "zoom".to_string(),
        pdf_page_widgets,
        pdf_zoom,
        pdf_render_cache,
        pdf_render_source,
        pdf_rerender_source,
    );
    queue_pdf_selection_draw(pdf_page_widgets);
    true
}

fn viewport_center(scroller: &gtk::ScrolledWindow) -> (f64, f64) {
    (
        f64::from(scroller.allocated_width().max(1)) / 2.0,
        f64::from(scroller.allocated_height().max(1)) / 2.0,
    )
}

fn keep_pdf_anchor_at_pointer(
    scroller: &gtk::ScrolledWindow,
    old_zoom: f64,
    new_zoom: f64,
    anchor: (f64, f64),
) {
    let factor = (new_zoom / old_zoom).max(0.01);
    let hadjustment = scroller.hadjustment();
    let vadjustment = scroller.vadjustment();
    let target_x = (hadjustment.value() + anchor.0) * factor - anchor.0;
    let target_y = (vadjustment.value() + anchor.1) * factor - anchor.1;

    set_adjustment_value_clamped(&hadjustment, target_x);
    set_adjustment_value_clamped(&vadjustment, target_y);

    gtk::glib::idle_add_local_once(move || {
        set_adjustment_value_clamped(&hadjustment, target_x);
        set_adjustment_value_clamped(&vadjustment, target_y);
    });
}

fn set_adjustment_value_clamped(adjustment: &gtk::Adjustment, value: f64) {
    let max = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
    adjustment.set_value(value.clamp(adjustment.lower(), max));
}

fn schedule_poppler_render(
    bytes: Arc<Vec<u8>>,
    identity: PdfPreviewIdentity,
    page_indices: Vec<i32>,
    pdf_generation: Rc<Cell<u64>>,
    generation: u64,
    zoom: f64,
    file_path: String,
    pdf_page_widgets: Rc<RefCell<Vec<PdfPageWidget>>>,
    pdf_zoom: Rc<Cell<f64>>,
    pdf_render_cache: Rc<RefCell<PdfRenderCache>>,
    pdf_render_source: Rc<RefCell<Option<gtk::glib::SourceId>>>,
) {
    cancel_pdf_render_receiver(&pdf_render_source);

    if page_indices.is_empty() {
        log::info!(
            "PDF preview render satisfied from cache path={file_path} zoom={zoom:.2} generation={generation}"
        );
        return;
    }

    let page_count = page_indices.len();
    let started = Instant::now();
    log::info!(
        "PDF preview background render scheduled path={file_path} pages={page_count} zoom={zoom:.2} generation={generation}"
    );

    let (sender, receiver) = mpsc::channel();
    spawn_pdf_render_workers(bytes, page_indices, zoom, sender);
    let render_source_for_poll = Rc::clone(&pdf_render_source);
    let source = gtk::glib::timeout_add_local(
        Duration::from_millis(PDF_RENDER_POLL_INTERVAL_MS),
        move || {
            if pdf_generation.get() != generation {
                log::info!(
                    "PDF preview render canceled path={file_path} generation={generation} current_generation={}",
                    pdf_generation.get()
                );
                render_source_for_poll.borrow_mut().take();
                return gtk::glib::ControlFlow::Break;
            }

            let mut processed = 0;
            loop {
                match receiver.try_recv() {
                    Ok(PdfRenderResult::Page(Ok(pixels))) => {
                        let page = rendered_pdf_page_from_pixels(pixels);
                        let key = pdf_render_cache_key(identity, page.page_index, zoom);
                        pdf_render_cache.borrow_mut().insert(key, page.clone());
                        apply_rendered_pdf_page(&pdf_page_widgets, &pdf_zoom, &page);
                    }
                    Ok(PdfRenderResult::Page(Err(err))) => {
                        mark_pdf_page_render_error(&pdf_page_widgets, err.page_index, &err.error);
                        log::warn!(
                            "PDF preview page render failed path={file_path} page={} zoom={zoom:.2} error={}",
                            err.page_index + 1,
                            err.error
                        );
                    }
                    Ok(PdfRenderResult::WorkerError(err)) => {
                        log::warn!("PDF preview render worker failed path={file_path}: {err}");
                    }
                    Err(TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
                    Err(TryRecvError::Disconnected) => {
                        log::info!(
                            "PDF preview background render complete path={file_path} pages={page_count} zoom={zoom:.2} elapsed_ms={}",
                            started.elapsed().as_millis()
                        );
                        render_source_for_poll.borrow_mut().take();
                        return gtk::glib::ControlFlow::Break;
                    }
                }

                processed += 1;
                if processed >= PDF_RENDER_RESULT_BATCH_LIMIT {
                    return gtk::glib::ControlFlow::Continue;
                }
            }
        },
    );
    pdf_render_source.replace(Some(source));
}

fn schedule_debounced_poppler_render(
    bytes: Arc<Vec<u8>>,
    identity: PdfPreviewIdentity,
    page_indices: Vec<i32>,
    pdf_generation: &Rc<Cell<u64>>,
    generation: u64,
    zoom: f64,
    file_path: String,
    pdf_page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    pdf_zoom: &Rc<Cell<f64>>,
    pdf_render_cache: &Rc<RefCell<PdfRenderCache>>,
    pdf_render_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
    pdf_rerender_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
) {
    cancel_pdf_rerender(pdf_rerender_source);

    if page_indices.is_empty() {
        log::debug!(
            "PDF preview zoom cache hit pages={} zoom={zoom:.2} generation={generation}",
            pdf_page_widgets.borrow().len()
        );
        return;
    }

    let pdf_generation = Rc::clone(pdf_generation);
    let pdf_page_widgets = Rc::clone(pdf_page_widgets);
    let pdf_zoom = Rc::clone(pdf_zoom);
    let pdf_render_cache = Rc::clone(pdf_render_cache);
    let pdf_render_source = Rc::clone(pdf_render_source);
    let pdf_rerender_source_for_timeout = Rc::clone(pdf_rerender_source);

    let source = gtk::glib::timeout_add_local(
        Duration::from_millis(PDF_ZOOM_RERENDER_DELAY_MS),
        move || {
            pdf_rerender_source_for_timeout.borrow_mut().take();
            if pdf_generation.get() != generation {
                return gtk::glib::ControlFlow::Break;
            }

            schedule_poppler_render(
                Arc::clone(&bytes),
                identity,
                page_indices.clone(),
                Rc::clone(&pdf_generation),
                generation,
                zoom,
                file_path.clone(),
                Rc::clone(&pdf_page_widgets),
                Rc::clone(&pdf_zoom),
                Rc::clone(&pdf_render_cache),
                Rc::clone(&pdf_render_source),
            );
            gtk::glib::ControlFlow::Break
        },
    );
    pdf_rerender_source.replace(Some(source));
}

fn cancel_pdf_rerender(pdf_rerender_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>) {
    if let Some(source) = pdf_rerender_source.borrow_mut().take() {
        source.remove();
    }
}

fn cancel_pdf_render_receiver(pdf_render_source: &Rc<RefCell<Option<gtk::glib::SourceId>>>) {
    if let Some(source) = pdf_render_source.borrow_mut().take() {
        source.remove();
    }
}

fn spawn_pdf_render_workers(
    bytes: Arc<Vec<u8>>,
    page_indices: Vec<i32>,
    zoom: f64,
    sender: Sender<PdfRenderResult>,
) {
    let worker_count = pdf_render_worker_count(page_indices.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(page_indices)));

    for _ in 0..worker_count {
        let bytes = Arc::clone(&bytes);
        let queue = Arc::clone(&queue);
        let sender = sender.clone();
        thread::spawn(move || run_pdf_render_worker(bytes, queue, zoom, sender));
    }
}

fn pdf_render_worker_count(page_count: usize) -> usize {
    let parallelism = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    page_count
        .min(PDF_RENDER_WORKER_LIMIT)
        .min(parallelism)
        .max(1)
}

fn run_pdf_render_worker(
    bytes: Arc<Vec<u8>>,
    queue: Arc<Mutex<VecDeque<i32>>>,
    zoom: f64,
    sender: Sender<PdfRenderResult>,
) {
    let document_bytes = gtk::glib::Bytes::from_owned(PdfByteStore(bytes));
    let document = match Document::from_bytes(&document_bytes, None) {
        Ok(document) => document,
        Err(err) => {
            let _ = sender.send(PdfRenderResult::WorkerError(format!(
                "Unable to load PDF in render worker: {err}"
            )));
            return;
        }
    };

    loop {
        let page_index = match queue.lock() {
            Ok(mut queue) => queue.pop_front(),
            Err(_) => {
                let _ = sender.send(PdfRenderResult::WorkerError(
                    "Unable to read render queue.".to_string(),
                ));
                return;
            }
        };
        let Some(page_index) = page_index else {
            return;
        };

        let result = render_poppler_page_pixels(&document, page_index, zoom)
            .map_err(|error| PdfPageRenderError { page_index, error });
        if sender.send(PdfRenderResult::Page(result)).is_err() {
            return;
        }
    }
}

fn render_poppler_page_pixels(
    document: &Document,
    index: i32,
    zoom: f64,
) -> Result<RenderedPdfPagePixels, String> {
    let page = document
        .page(index)
        .ok_or_else(|| "Page does not exist.".to_string())?;
    let (page_width, page_height) = page.size();
    let base_scale = (PDF_BASE_MAX_WIDTH / page_width.max(1.0)).min(1.0);
    let scale = base_scale * zoom;
    let width = (page_width * scale).ceil().max(1.0) as i32;
    let height = (page_height * scale).ceil().max(1.0) as i32;

    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
        .map_err(|err| format!("Unable to create page surface: {err}"))?;
    let context = cairo::Context::new(&surface)
        .map_err(|err| format!("Unable to create Cairo context: {err}"))?;
    context.set_source_rgb(1.0, 1.0, 1.0);
    context
        .paint()
        .map_err(|err| format!("Unable to paint page background: {err}"))?;
    context.scale(scale, scale);
    page.render(&context);
    drop(context);
    surface.flush();

    let stride = surface.stride() as usize;
    let data = surface
        .take_data()
        .map_err(|err| format!("Unable to read page pixels: {err}"))?;
    let pixels = data.to_vec();

    log::debug!(
        "PDF preview rendered page={} width={width} height={height} zoom={zoom:.2}",
        index + 1
    );

    Ok(RenderedPdfPagePixels {
        page_index: index,
        width,
        height,
        stride,
        pixels,
    })
}

fn rendered_pdf_page_from_pixels(page: RenderedPdfPagePixels) -> RenderedPdfPage {
    let byte_size = page.pixels.len();
    let bytes = gtk::glib::Bytes::from_owned(page.pixels);
    let texture = gdk::MemoryTexture::new(
        page.width,
        page.height,
        gdk::MemoryFormat::B8g8r8a8Premultiplied,
        &bytes,
        page.stride,
    );

    RenderedPdfPage {
        texture,
        page_index: page.page_index,
        byte_size,
    }
}

fn build_pdf_page_placeholders(
    document: &Document,
    pages: &gtk::Box,
    scroller: &gtk::ScrolledWindow,
    page_widgets: Rc<RefCell<Vec<PdfPageWidget>>>,
    selection: Rc<RefCell<Option<PdfTextSelection>>>,
    pdf_zoom: Rc<Cell<f64>>,
    autoscroll: Rc<canvas_scroll::MiddleAutoscroll>,
) {
    clear_pdf_pages(pages);
    page_widgets.borrow_mut().clear();

    for page_index in 0..document.n_pages() {
        let Some(page) = document.page(page_index) else {
            continue;
        };
        let (page_width, page_height) = page.size();
        let base_scale = (PDF_BASE_MAX_WIDTH / page_width.max(1.0)).min(1.0);
        append_pdf_page_placeholder(
            pages,
            document,
            scroller,
            &page_widgets,
            &selection,
            &pdf_zoom,
            &autoscroll,
            &page,
            page_index,
            page_width,
            page_height,
            base_scale,
        );
    }
}

fn append_pdf_page_placeholder(
    pages: &gtk::Box,
    document: &Document,
    scroller: &gtk::ScrolledWindow,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    pdf_zoom: &Rc<Cell<f64>>,
    autoscroll: &Rc<canvas_scroll::MiddleAutoscroll>,
    page: &Page,
    page_index: i32,
    page_width: f64,
    page_height: f64,
    base_scale: f64,
) {
    let scale = (base_scale * pdf_zoom.get()).max(0.01);
    let width = (page_width * scale).ceil().max(1.0) as i32;
    let height = (page_height * scale).ceil().max(1.0) as i32;

    let picture = gtk::Picture::builder()
        .can_shrink(false)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .build();
    picture.set_content_fit(gtk::ContentFit::Fill);
    picture.set_size_request(width, height);

    let selection_area = gtk::DrawingArea::builder()
        .content_width(width)
        .content_height(height)
        .hexpand(true)
        .vexpand(true)
        .focusable(false)
        .can_target(true)
        .build();
    let page_for_draw = page.clone();
    let selection_for_draw = Rc::clone(selection);
    let pdf_zoom_for_draw = Rc::clone(pdf_zoom);
    selection_area.set_draw_func({
        move |_, context, _, _| {
            draw_pdf_selection(
                &page_for_draw,
                context,
                page_index,
                page_width,
                page_height,
                base_scale,
                pdf_zoom_for_draw.get(),
                &selection_for_draw,
            );
        }
    });

    let overlay = gtk::Overlay::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .build();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(&selection_area);
    overlay.set_size_request(width, height);

    let frame = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Center)
        .margin_start(PDF_PAGE_SHADOW_MARGIN)
        .margin_end(PDF_PAGE_SHADOW_MARGIN)
        .build();
    frame.add_css_class("pdf-preview-page");
    frame.append(&overlay);
    install_pdf_selection_handlers(
        &selection_area,
        document,
        page_index,
        page_width,
        page_height,
        base_scale,
        scroller,
        pdf_zoom,
        selection,
        page_widgets,
        autoscroll,
    );

    let widget = PdfPageWidget {
        page_index,
        picture,
        overlay,
        selection_area,
        page_width,
        page_height,
        base_scale,
    };
    widget.set_display_zoom(pdf_zoom.get());
    page_widgets.borrow_mut().push(widget);
    pages.append(&frame);
}

fn page_indices_for_count(page_count: i32) -> Vec<i32> {
    (0..page_count).collect()
}

fn pdf_render_cache_key(
    identity: PdfPreviewIdentity,
    page_index: i32,
    zoom: f64,
) -> PdfRenderCacheKey {
    PdfRenderCacheKey {
        identity,
        page_index,
        zoom_key: (zoom * 1000.0).round().max(0.0) as u32,
    }
}

fn apply_cached_pdf_pages(
    render_cache: &Rc<RefCell<PdfRenderCache>>,
    identity: PdfPreviewIdentity,
    zoom: f64,
    page_count: i32,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    pdf_zoom: &Rc<Cell<f64>>,
) -> Vec<i32> {
    let mut missing_pages = Vec::new();
    let mut render_cache = render_cache.borrow_mut();

    for page_index in 0..page_count {
        let key = pdf_render_cache_key(identity, page_index, zoom);
        if let Some(page) = render_cache.get(key) {
            apply_rendered_pdf_page(page_widgets, pdf_zoom, &page);
        } else {
            missing_pages.push(page_index);
        }
    }

    missing_pages
}

fn apply_rendered_pdf_page(
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    pdf_zoom: &Rc<Cell<f64>>,
    page: &RenderedPdfPage,
) {
    let Some(widget) = page_widgets
        .borrow()
        .iter()
        .find(|widget| widget.page_index == page.page_index)
        .cloned()
    else {
        return;
    };

    widget.picture.set_tooltip_text(None);
    widget.picture.set_paintable(Some(&page.texture));
    widget.set_display_zoom(pdf_zoom.get());
}

fn mark_pdf_page_render_error(
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    page_index: i32,
    error: &str,
) {
    let tooltip = format!("Unable to render page {}: {error}", page_index + 1);
    if let Some(widget) = page_widgets
        .borrow()
        .iter()
        .find(|widget| widget.page_index == page_index)
    {
        widget.picture.set_tooltip_text(Some(&tooltip));
    }
}

fn install_pdf_selection_handlers(
    area: &gtk::DrawingArea,
    document: &Document,
    page_index: i32,
    page_width: f64,
    page_height: f64,
    base_scale: f64,
    scroller: &gtk::ScrolledWindow,
    pdf_zoom: &Rc<Cell<f64>>,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    autoscroll: &Rc<canvas_scroll::MiddleAutoscroll>,
) {
    let click = gtk::GestureClick::builder().button(1).build();
    click.connect_pressed({
        let scroller = scroller.clone();
        let selection = Rc::clone(selection);
        let page_widgets = Rc::clone(page_widgets);
        move |_, _, _, _| {
            scroller.grab_focus();
            selection.borrow_mut().take();
            queue_pdf_selection_draw(&page_widgets);
        }
    });
    area.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter({
        let area = area.clone();
        let autoscroll = Rc::clone(autoscroll);
        move |_, _, _| {
            if !autoscroll.is_active() {
                area.set_cursor_from_name(Some("text"));
            }
        }
    });
    motion.connect_leave({
        let area = area.clone();
        let autoscroll = Rc::clone(autoscroll);
        move |_| {
            if !autoscroll.is_active() {
                area.set_cursor_from_name(None);
            }
        }
    });
    area.add_controller(motion);

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    let drag_start = Rc::new(Cell::new(None::<PdfSelectionPoint>));
    drag.connect_drag_begin({
        let scroller = scroller.clone();
        let pdf_zoom = Rc::clone(pdf_zoom);
        let drag_start = Rc::clone(&drag_start);
        move |_, x, y| {
            scroller.grab_focus();
            drag_start.set(Some(PdfSelectionPoint {
                page_index,
                point: pdf_point_for_widget(
                    x,
                    y,
                    page_width,
                    page_height,
                    base_scale,
                    pdf_zoom.get(),
                ),
            }));
        }
    });
    drag.connect_drag_update({
        let area = area.clone();
        let document = document.clone();
        let pdf_zoom = Rc::clone(pdf_zoom);
        let selection = Rc::clone(selection);
        let page_widgets = Rc::clone(page_widgets);
        let drag_start = Rc::clone(&drag_start);
        move |gesture, offset_x, offset_y| {
            let Some(start) = drag_start.get() else {
                return;
            };
            let Some((start_x, start_y)) = gesture.start_point() else {
                return;
            };
            let Some(end) = pdf_selection_point_for_widget_point(
                &area,
                &page_widgets,
                start_x + offset_x,
                start_y + offset_y,
                pdf_zoom.get(),
            ) else {
                return;
            };
            set_pdf_selection(&document, start, end, &selection, &page_widgets);
        }
    });
    drag.connect_drag_end({
        let drag_start = Rc::clone(&drag_start);
        move |_, _, _| drag_start.set(None)
    });
    area.add_controller(drag);
}

fn pdf_selection_point_for_widget_point(
    origin_area: &gtk::DrawingArea,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
    x: f64,
    y: f64,
    zoom: f64,
) -> Option<PdfSelectionPoint> {
    let mut best = None::<(f64, PdfSelectionPoint)>;

    for widget in page_widgets.borrow().iter() {
        let Some((page_x, page_y)) =
            origin_area.translate_coordinates(&widget.selection_area, x, y)
        else {
            continue;
        };
        let scale = (widget.base_scale * zoom).max(0.01);
        let page_display_width = (widget.page_width * scale).ceil().max(1.0);
        let page_display_height = (widget.page_height * scale).ceil().max(1.0);
        let vertical_distance = if page_y < 0.0 {
            -page_y
        } else if page_y > page_display_height {
            page_y - page_display_height
        } else {
            0.0
        };
        let point = pdf_point_for_widget(
            page_x.clamp(0.0, page_display_width),
            page_y.clamp(0.0, page_display_height),
            widget.page_width,
            widget.page_height,
            widget.base_scale,
            zoom,
        );
        let candidate = PdfSelectionPoint {
            page_index: widget.page_index,
            point,
        };

        if best
            .as_ref()
            .is_none_or(|(best_distance, _)| vertical_distance < *best_distance)
        {
            best = Some((vertical_distance, candidate));
        }
    }

    best.map(|(_, point)| point)
}

fn set_pdf_selection(
    document: &Document,
    start: PdfSelectionPoint,
    end: PdfSelectionPoint,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
) {
    if start.page_index == end.page_index
        && (start.point.x - end.point.x).abs() < PDF_SELECTION_MIN_DISTANCE
        && (start.point.y - end.point.y).abs() < PDF_SELECTION_MIN_DISTANCE
    {
        selection.borrow_mut().take();
        queue_pdf_selection_draw(page_widgets);
        return;
    }

    let kind = PdfTextSelectionKind::Range { start, end };
    let text = pdf_text_for_selection(document, &kind);
    selection.replace(Some(PdfTextSelection { kind, text }));
    queue_pdf_selection_draw(page_widgets);
}

fn draw_pdf_selection(
    page: &Page,
    context: &cairo::Context,
    page_index: i32,
    page_width: f64,
    page_height: f64,
    base_scale: f64,
    zoom: f64,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
) {
    let selection = selection.borrow();
    let Some(selection) = selection.as_ref() else {
        return;
    };
    let Some(mut rect) =
        pdf_selection_rectangle_for_page(&selection.kind, page_index, page_width, page_height)
    else {
        return;
    };

    let scale = (base_scale * zoom).max(0.01);
    let Some(region) = page.selected_region(scale, SelectionStyle::Glyph, &mut rect) else {
        return;
    };

    context.set_source_rgba(0.19, 0.45, 0.95, 0.28);
    for index in 0..region.num_rectangles() {
        let rect = region.rectangle(index);
        context.rectangle(
            f64::from(rect.x()),
            f64::from(rect.y()),
            f64::from(rect.width()),
            f64::from(rect.height()),
        );
    }
    let _ = context.fill();
}

fn pdf_selection_rectangle_for_page(
    kind: &PdfTextSelectionKind,
    page_index: i32,
    page_width: f64,
    page_height: f64,
) -> Option<Rectangle> {
    match kind {
        PdfTextSelectionKind::AllPages => Some(full_page_rectangle(page_width, page_height)),
        PdfTextSelectionKind::Range { start, end } => {
            range_selection_rectangle_for_page(*start, *end, page_index, page_width, page_height)
        }
    }
}

fn range_selection_rectangle_for_page(
    start: PdfSelectionPoint,
    end: PdfSelectionPoint,
    page_index: i32,
    page_width: f64,
    page_height: f64,
) -> Option<Rectangle> {
    let (first, last) = ordered_pdf_selection_points(start, end);

    if page_index < first.page_index || page_index > last.page_index {
        return None;
    }

    if first.page_index == last.page_index {
        return Some(selection_rectangle(
            clamp_pdf_point(first.point, page_width, page_height),
            clamp_pdf_point(last.point, page_width, page_height),
        ));
    }

    if page_index == first.page_index {
        return Some(selection_rectangle(
            clamp_pdf_point(first.point, page_width, page_height),
            PdfPoint {
                x: page_width,
                y: page_height,
            },
        ));
    }

    if page_index == last.page_index {
        return Some(selection_rectangle(
            PdfPoint { x: 0.0, y: 0.0 },
            clamp_pdf_point(last.point, page_width, page_height),
        ));
    }

    Some(full_page_rectangle(page_width, page_height))
}

fn ordered_pdf_selection_points(
    start: PdfSelectionPoint,
    end: PdfSelectionPoint,
) -> (PdfSelectionPoint, PdfSelectionPoint) {
    if start.page_index <= end.page_index {
        (start, end)
    } else {
        (end, start)
    }
}

fn full_page_rectangle(page_width: f64, page_height: f64) -> Rectangle {
    let mut rect = Rectangle::new();
    rect.set_x1(0.0);
    rect.set_y1(0.0);
    rect.set_x2(page_width);
    rect.set_y2(page_height);
    rect
}

fn selection_rectangle(start: PdfPoint, end: PdfPoint) -> Rectangle {
    let mut rect = Rectangle::new();
    rect.set_x1(start.x.min(end.x));
    rect.set_y1(start.y.min(end.y));
    rect.set_x2(start.x.max(end.x));
    rect.set_y2(start.y.max(end.y));
    rect
}

fn pdf_point_for_widget(
    x: f64,
    y: f64,
    page_width: f64,
    page_height: f64,
    base_scale: f64,
    zoom: f64,
) -> PdfPoint {
    let scale = (base_scale * zoom).max(0.01);
    clamp_pdf_point(
        PdfPoint {
            x: x / scale,
            y: y / scale,
        },
        page_width,
        page_height,
    )
}

fn clamp_pdf_point(point: PdfPoint, page_width: f64, page_height: f64) -> PdfPoint {
    PdfPoint {
        x: point.x.clamp(0.0, page_width),
        y: point.y.clamp(0.0, page_height),
    }
}

fn copy_pdf_selection(selection: &Rc<RefCell<Option<PdfTextSelection>>>) -> bool {
    let text = pdf_selection_clipboard_text(selection);
    let Some(text) = text else {
        return false;
    };
    let Some(display) = gdk::Display::default() else {
        return false;
    };

    display.clipboard().set_text(&text);
    true
}

fn pdf_selection_clipboard_text(
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
) -> Option<String> {
    selection
        .borrow()
        .as_ref()
        .map(|selection| selection.text.trim_matches('\0').trim().to_string())
        .filter(|text| !text.is_empty())
}

fn select_all_pdf_text(
    document: &Rc<RefCell<Option<Document>>>,
    selection: &Rc<RefCell<Option<PdfTextSelection>>>,
    page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>,
) -> bool {
    let Some(document) = document.borrow().clone() else {
        return false;
    };

    let kind = PdfTextSelectionKind::AllPages;
    let text = pdf_text_for_selection(&document, &kind);
    if text.is_empty() {
        return false;
    }

    selection.replace(Some(PdfTextSelection { kind, text }));
    queue_pdf_selection_draw(page_widgets);
    true
}

fn pdf_text_for_selection(document: &Document, kind: &PdfTextSelectionKind) -> String {
    let page_count = document.n_pages();
    if page_count <= 0 {
        return String::new();
    }

    let (start_page, end_page) = match kind {
        PdfTextSelectionKind::AllPages => (0, page_count.saturating_sub(1)),
        PdfTextSelectionKind::Range { start, end } => {
            let (first, last) = ordered_pdf_selection_points(*start, *end);
            (
                first.page_index.clamp(0, page_count.saturating_sub(1)),
                last.page_index.clamp(0, page_count.saturating_sub(1)),
            )
        }
    };
    if start_page > end_page {
        return String::new();
    }

    (start_page..=end_page)
        .filter_map(|page_index| {
            let page = document.page(page_index)?;
            let (page_width, page_height) = page.size();
            let mut rect =
                pdf_selection_rectangle_for_page(kind, page_index, page_width, page_height)?;
            page.selected_text(SelectionStyle::Glyph, &mut rect)
        })
        .map(|text| text.trim_matches('\0').trim().to_string())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_pdf_zoom_to_widgets(page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>, zoom: f64) {
    for widget in page_widgets.borrow().iter() {
        widget.set_display_zoom(zoom);
    }
}

fn queue_pdf_selection_draw(page_widgets: &Rc<RefCell<Vec<PdfPageWidget>>>) {
    for widget in page_widgets.borrow().iter() {
        widget.selection_area.queue_draw();
    }
}

fn clear_pdf_pages(pages: &gtk::Box) {
    while let Some(child) = pages.first_child() {
        pages.remove(&child);
    }
}
