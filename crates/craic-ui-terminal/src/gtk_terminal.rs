use crate::ui::components::terminal::{TerminalActivation, TerminalFileActivation};
use craic_platform::terminal::{TerminalBackend, TerminalCommand, TerminalLifecycle};
use craic_render_skia::{
    TerminalClipboard, TerminalKey, TerminalKeyAction, TerminalKeyEvent, TerminalKeyModifiers,
    TerminalLink, TerminalMouseAction, TerminalMouseButton, TerminalMouseModifiers,
    TerminalPaintCache, TerminalPaintRequest, TerminalScroll, TerminalSearchDirection,
    TerminalSelectionType, TerminalSession, TerminalSide, TerminalViewport, paint_terminal,
};
use craic_ui_core::ui::{canvas_scroll, components::context_menu};
use glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::rc::Rc;
use std::time::Duration;

type Handler<T> = Rc<dyn Fn(T)>;
type FileDropHandler = Rc<dyn Fn(GtkTerminal, Vec<PathBuf>)>;

#[derive(Clone, Copy)]
struct KeyInput {
    code: u32,
    key: TerminalKey,
    action: TerminalKeyAction,
    modifiers: TerminalKeyModifiers,
    consumed_modifiers: TerminalKeyModifiers,
    unshifted: Option<char>,
}

#[derive(Clone)]
pub struct GtkTerminal(Rc<View>);

struct View {
    root: gtk::Overlay,
    canvas: gtk::DrawingArea,
    adjustment: gtk::Adjustment,
    adjusting: Cell<bool>,
    paint: RefCell<TerminalPaintCache>,
    font_size: Cell<f32>,
    viewport: Cell<TerminalViewport>,
    session: RefCell<Option<TerminalSession>>,
    spawning: Cell<bool>,
    terminated: Cell<bool>,
    pressed_keys: RefCell<HashMap<u32, (TerminalKey, Option<char>)>>,
    ime_key: Cell<Option<KeyInput>>,
    preedit: RefCell<String>,
    mouse_button: Cell<TerminalMouseButton>,
    pointer: Cell<(f64, f64)>,
    selecting: Cell<bool>,
    launch_dir: RefCell<String>,
    title: RefCell<Option<String>>,
    exit_handlers: RefCell<Vec<Handler<ExitStatus>>>,
    title_handlers: RefCell<Vec<Handler<String>>>,
    activity_handlers: RefCell<Vec<Handler<bool>>>,
    activation_handlers: RefCell<Vec<Handler<TerminalActivation>>>,
    file_drop_handler: RefCell<Option<FileDropHandler>>,
    session_controllers: RefCell<Vec<gtk::EventController>>,
    window_lifetime: RefCell<Option<WindowLifetime>>,
}

struct WindowLifetime {
    window: glib::WeakRef<gtk::Window>,
    application: glib::WeakRef<gtk::Application>,
    removed: Option<glib::SignalHandlerId>,
}

impl Drop for WindowLifetime {
    fn drop(&mut self) {
        if let (Some(application), Some(removed)) =
            (self.application.upgrade(), self.removed.take())
        {
            application.disconnect(removed);
        }
    }
}

impl GtkTerminal {
    fn lifecycle(&self) -> TerminalLifecycle {
        if self.0.terminated.get() {
            TerminalLifecycle::Closing
        } else if self.0.spawning.get() {
            TerminalLifecycle::Starting
        } else {
            self.0
                .session
                .borrow()
                .as_ref()
                .map_or(TerminalLifecycle::Exited, TerminalSession::lifecycle)
        }
    }

    fn send_input(&self, bytes: &[u8]) -> Result<(), String> {
        let session = self.0.session.borrow();
        let session = session.as_ref().ok_or("Terminal process is not running.")?;
        session.send_input(bytes)?;
        session.scroll(TerminalScroll::Bottom);
        self.0.refresh();
        Ok(())
    }

    pub fn terminate(&self) {
        if self.0.terminated.replace(true) {
            return;
        }
        self.0.window_lifetime.take();
        for controller in self.0.session_controllers.take() {
            self.0.canvas.remove_controller(&controller);
        }
        self.0.exit_handlers.take();
        self.0.title_handlers.take();
        self.0.activity_handlers.take();
        self.0.activation_handlers.take();
        self.0.file_drop_handler.take();
        if let Some(mut session) = self.0.session.take() {
            session.begin_shutdown();
            std::thread::spawn(move || {
                if let Err(error) = session.shutdown() {
                    log::warn!("terminal shutdown failed: {error}");
                }
            });
        }
    }

    pub fn new(font_size: f64) -> Self {
        let font_size = (font_size * 4.0 / 3.0) as f32;
        let mut paint = TerminalPaintCache::new(font_size);
        let (cell_width, cell_height) = paint.cell_size(font_size);
        let viewport = TerminalViewport::new(100, 34, cell_width, cell_height);
        let session = TerminalSession::new(viewport)
            .inspect_err(|error| {
                log::error!("could not create Ghostty terminal: {error}");
            })
            .ok();
        let canvas = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .focusable(true)
            .build();
        let adjustment = gtk::Adjustment::new(
            0.0,
            0.0,
            viewport.lines as f64,
            1.0,
            viewport.lines as f64,
            viewport.lines as f64,
        );
        let scrollbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, Some(&adjustment));
        let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.append(&canvas);
        row.append(&scrollbar);
        root.set_child(Some(&row));
        let terminal = Self(Rc::new(View {
            root,
            canvas,
            adjustment,
            paint: RefCell::new(paint),
            font_size: Cell::new(font_size),
            viewport: Cell::new(viewport),
            adjusting: Cell::new(false),
            session: RefCell::new(session),
            spawning: Cell::new(false),
            terminated: Cell::new(false),
            pressed_keys: RefCell::new(HashMap::new()),
            ime_key: Cell::new(None),
            preedit: RefCell::new(String::new()),
            mouse_button: Cell::new(TerminalMouseButton::None),
            pointer: Cell::new((0.0, 0.0)),
            selecting: Cell::new(false),
            launch_dir: RefCell::new(String::new()),
            title: RefCell::new(None),
            exit_handlers: RefCell::new(Vec::new()),
            title_handlers: RefCell::new(Vec::new()),
            activity_handlers: RefCell::new(Vec::new()),
            activation_handlers: RefCell::new(Vec::new()),
            file_drop_handler: RefCell::new(None),
            session_controllers: RefCell::new(Vec::new()),
            window_lifetime: RefCell::new(None),
        }));
        terminal.0.root.connect_root_notify({
            let view = Rc::downgrade(&terminal.0);
            move |widget| {
                // Page navigation unparents the pane. Keep watching its
                // owning window even while the terminal is off-page.
                let Some(view) = view.upgrade().filter(|view| !view.terminated.get()) else {
                    return;
                };
                let Some(window) = widget.root().and_downcast::<gtk::Window>() else {
                    return;
                };
                let Some(application) = window.application() else {
                    return;
                };
                if view
                    .window_lifetime
                    .borrow()
                    .as_ref()
                    .is_some_and(|lifetime| {
                        lifetime.window.upgrade().as_ref() == Some(&window)
                            && lifetime.application.upgrade().as_ref() == Some(&application)
                    })
                {
                    return;
                }
                let window = window.downgrade();
                let removed = application.connect_window_removed({
                    let window = window.clone();
                    let view = Rc::downgrade(&view);
                    move |_, removed| {
                        if window.upgrade().as_ref() == Some(removed)
                            && let Some(view) = view.upgrade()
                        {
                            log::debug!(
                                "closing terminal because its application window was removed"
                            );
                            GtkTerminal(view).terminate();
                        }
                    }
                });
                view.window_lifetime.replace(Some(WindowLifetime {
                    window,
                    application: application.downgrade(),
                    removed: Some(removed),
                }));
            }
        });
        terminal.install_canvas();
        terminal.install_input();
        terminal.install_autoscroll();
        terminal.install_context_menu();
        terminal.install_file_drop();
        terminal
    }

    pub fn widget(&self) -> gtk::Widget {
        self.0.root.clone().upcast()
    }

    pub fn add_controller(&self, controller: impl IsA<gtk::EventController>) {
        let controller = controller.upcast::<gtk::EventController>();
        self.0
            .session_controllers
            .borrow_mut()
            .push(controller.clone());
        self.0.canvas.add_controller(controller);
    }

    pub fn grab_focus(&self) {
        self.0.canvas.grab_focus();
    }

    pub fn spawn<F>(&self, command: TerminalCommand, callback: F) -> Result<(), String>
    where
        F: FnOnce(Result<i32, String>) + 'static,
    {
        if self.lifecycle() != TerminalLifecycle::Exited {
            return Err("Terminal process has already been started.".to_owned());
        }
        command.utf8_argv()?;
        let viewport = self.0.viewport.get();
        self.0
            .launch_dir
            .replace(command.target_working_directory.clone());
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("terminal-launch".into())
            .spawn(move || {
                let result = TerminalSession::spawn(command, viewport);
                // Dropping an undeliverable session cleans up the child on this worker.
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        self.0.spawning.set(true);

        let terminal = Rc::downgrade(&self.0);
        let mut callback = Some(callback);
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let Ok(result) = receiver.try_recv().or_else(|error| {
                if error == std::sync::mpsc::TryRecvError::Disconnected {
                    Ok(Err(std::io::Error::other("terminal launch worker stopped")))
                } else {
                    Err(error)
                }
            }) else {
                return glib::ControlFlow::Continue;
            };
            let result = match result {
                Ok(mut session) => {
                    let pid = session.child_pid() as i32;
                    if let Some(view) = terminal.upgrade().filter(|view| !view.terminated.get()) {
                        view.spawning.set(false);
                        if let Err(error) = session.resize(view.viewport.get()) {
                            log::warn!("terminal initial resize failed: {error}");
                        }
                        let mut wake = session.subscribe();
                        view.session.replace(Some(session));
                        view.refresh();
                        let weak = Rc::downgrade(&view);
                        glib::MainContext::default().spawn_local(async move {
                            loop {
                                let Some(view) =
                                    weak.upgrade().filter(|view| !view.terminated.get())
                                else {
                                    break;
                                };
                                let delay = view.drain_events();
                                drop(view);
                                if let Some(delay) = delay {
                                    if matches!(
                                        glib::future_with_timeout(delay, wake.changed()).await,
                                        Ok(Err(_))
                                    ) {
                                        break;
                                    }
                                } else if wake.changed().await.is_err() {
                                    break;
                                }
                            }
                        });
                        log::debug!("terminal PTY started pid={pid}");
                    } else {
                        session.begin_shutdown();
                        std::thread::spawn(move || {
                            let _ = session.shutdown();
                        });
                    }
                    Ok(pid)
                }
                Err(error) => {
                    if let Some(view) = terminal.upgrade() {
                        view.spawning.set(false);
                    }
                    Err(error.to_string())
                }
            };
            if let Some(callback) = callback.take() {
                callback(result);
            }
            glib::ControlFlow::Break
        });
        Ok(())
    }

    pub fn connect_child_exited(&self, callback: impl Fn(ExitStatus) + 'static) {
        self.0.exit_handlers.borrow_mut().push(Rc::new(callback));
    }

    pub fn connect_title_changed(&self, callback: impl Fn(String) + 'static) {
        self.0.title_handlers.borrow_mut().push(Rc::new(callback));
    }

    pub fn connect_reported_activity_changed(&self, callback: impl Fn(bool) + 'static) {
        self.0
            .activity_handlers
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub fn connect_activation(&self, callback: impl Fn(TerminalActivation) + 'static) {
        self.0
            .activation_handlers
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub fn title(&self) -> Option<String> {
        self.0.title.borrow().clone()
    }

    pub fn foreground_process_group(&self) -> Option<i32> {
        self.0.session.borrow().as_ref()?.foreground_process_group()
    }

    pub fn set_font_size(&self, font_size: f64) {
        self.0.font_size.set((font_size * 4.0 / 3.0) as f32);
        self.0.resize(self.0.canvas.width(), self.0.canvas.height());
        self.0.canvas.queue_draw();
    }

    pub fn has_selection(&self) -> bool {
        self.0
            .session
            .borrow()
            .as_ref()
            .and_then(TerminalSession::selected_text)
            .is_some_and(|text| !text.is_empty())
    }

    pub fn copy_clipboard(&self) {
        if let Some(text) = self
            .0
            .session
            .borrow()
            .as_ref()
            .and_then(TerminalSession::selected_text)
        {
            self.0.canvas.clipboard().set_text(&text);
        }
    }

    pub fn paste_clipboard(&self) {
        self.paste_from(&self.0.canvas.clipboard());
    }

    fn paste_from(&self, clipboard: &gdk::Clipboard) {
        let view = Rc::downgrade(&self.0);
        clipboard.read_text_async(None::<&gio::Cancellable>, move |result| {
            if let (Some(view), Ok(Some(text))) = (view.upgrade(), result) {
                GtkTerminal(view).paste_text(&text);
            }
        });
    }

    pub fn paste_text(&self, text: &str) {
        if let Some(session) = self.0.session.borrow().as_ref()
            && let Err(error) = session.paste(text)
            && session.lifecycle() == TerminalLifecycle::Running
        {
            log::warn!("terminal paste failed: {error}");
        }
        self.0.refresh();
    }

    pub fn set_file_drop_handler<F>(&self, handler: F)
    where
        F: Fn(GtkTerminal, Vec<PathBuf>) + 'static,
    {
        self.0.file_drop_handler.replace(Some(Rc::new(handler)));
    }

    pub fn paste_file_paths(&self, paths: &[PathBuf]) {
        let text = paths
            .iter()
            .map(|path| format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'")))
            .collect::<Vec<_>>()
            .join(" ");
        self.paste_text(&text);
        self.grab_focus();
    }

    pub fn feed_child(&self, bytes: &[u8]) {
        let _ = self.send_input(bytes);
    }

    pub fn feed(&self, bytes: &[u8]) {
        if let Some(session) = self.0.session.borrow().as_ref() {
            session.feed(bytes);
        }
        self.0.refresh();
    }

    pub fn search(&self, pattern: Option<&str>, backwards: bool) -> Result<bool, String> {
        let direction = if backwards {
            TerminalSearchDirection::Previous
        } else {
            TerminalSearchDirection::Next
        };
        let found = match self.0.session.borrow().as_ref() {
            Some(session) => session
                .search(pattern.unwrap_or_default(), direction)?
                .is_some(),
            None => false,
        };
        self.0.refresh();
        Ok(found)
    }

    pub fn all_text(&self) -> Option<String> {
        self.0
            .session
            .borrow()
            .as_ref()
            .map(TerminalSession::all_text)
    }

    pub fn cursor_row(&self) -> Option<i64> {
        Some(self.0.session.borrow().as_ref()?.cursor_row() as i64)
    }

    pub fn text_before_cursor(&self, max_lines: usize) -> Option<String> {
        Some(
            self.0
                .session
                .borrow()
                .as_ref()?
                .text_before_cursor(max_lines),
        )
    }

    pub fn recent_text(&self, max_lines: usize) -> Option<String> {
        Some(self.0.session.borrow().as_ref()?.recent_text(max_lines))
    }

    fn install_canvas(&self) {
        self.0.canvas.set_draw_func({
            let view = Rc::downgrade(&self.0);
            move |area, cr, width, height| {
                let Some(view) = view.upgrade() else { return };
                let viewport = view.viewport.get();
                let snapshot = view
                    .session
                    .borrow()
                    .as_ref()
                    .map(TerminalSession::snapshot);
                let Some(snapshot) = snapshot else { return };

                let scale = area.scale_factor().max(1);
                let (width_px, height_px) =
                    (width.saturating_mul(scale), height.saturating_mul(scale));
                let Ok(stride) = cairo::Format::ARgb32.stride_for_width(width_px as u32) else {
                    return;
                };
                let Some(bytes) = (stride as usize).checked_mul(height_px as usize) else {
                    return;
                };
                if bytes > 128 * 1024 * 1024 {
                    return;
                }
                let mut pixels = vec![0; bytes];
                let info = skia_safe::ImageInfo::new_n32_premul((width_px, height_px), None);
                {
                    let Some(mut surface) =
                        skia_safe::surfaces::wrap_pixels(&info, &mut pixels, stride as usize, None)
                    else {
                        return;
                    };
                    surface.canvas().scale((scale as f32, scale as f32));
                    paint_terminal(
                        surface.canvas(),
                        TerminalPaintRequest {
                            snapshot: &snapshot,
                            viewport_width: width as f32,
                            viewport_height: height as f32,
                            cell_width: f32::from(viewport.cell_width),
                            cell_height: f32::from(viewport.cell_height),
                            font_size: view.font_size.get(),
                            cursor_visible: true,
                            focused: area.has_focus(),
                            marked_text: Some(&view.preedit.borrow()),
                        },
                        &mut view.paint.borrow_mut(),
                    );
                }
                if let Ok(image) = cairo::ImageSurface::create_for_data(
                    pixels,
                    cairo::Format::ARgb32,
                    width_px,
                    height_px,
                    stride,
                ) {
                    image.set_device_scale(f64::from(scale), f64::from(scale));
                    if cr.set_source_surface(&image, 0.0, 0.0).is_ok() {
                        let _ = cr.paint();
                    }
                }
            }
        });
        self.0.canvas.connect_resize({
            let view = Rc::downgrade(&self.0);
            move |_, width, height| {
                if let Some(view) = view.upgrade() {
                    view.resize(width, height);
                }
            }
        });
        self.0.adjustment.connect_value_changed({
            let view = Rc::downgrade(&self.0);
            move |adjustment| {
                let Some(view) = view.upgrade().filter(|view| !view.adjusting.get()) else {
                    return;
                };
                if let Some(session) = view.session.borrow().as_ref() {
                    session.scroll(TerminalScroll::Row(adjustment.value().round() as usize));
                }
                view.canvas.queue_draw();
            }
        });
    }

    fn install_input(&self) {
        let keys = gtk::EventControllerKey::new();
        let input = gtk::IMMulticontext::new();
        input.set_client_widget(Some(&self.0.canvas));
        input.connect_preedit_changed({
            let view = Rc::downgrade(&self.0);
            move |input| {
                if let Some(view) = view.upgrade() {
                    view.preedit.replace(input.preedit_string().0.to_string());
                    view.ime_key.set(None);
                    view.canvas.queue_draw();
                }
            }
        });
        input.connect_commit({
            let view = Rc::downgrade(&self.0);
            move |_, text| {
                if let Some(view) = view.upgrade() {
                    if let Some(key) = view.ime_key.take()
                        && text.chars().count() == 1
                        && view.send_key(key, text)
                    {
                        return;
                    }
                    GtkTerminal(view).feed_child(text.as_bytes());
                }
            }
        });
        keys.connect_key_pressed({
            let view = Rc::downgrade(&self.0);
            let input = input.clone();
            move |controller, key, code, state| {
                let Some(view) = view.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let event = controller
                    .current_event()
                    .and_then(|event| event.downcast::<gdk::KeyEvent>().ok());
                let unshifted = event.as_ref().and_then(|event| {
                    view.canvas
                        .display()
                        .translate_key(code, gdk::ModifierType::empty(), event.layout() as i32)
                        .and_then(|(key, _, _, _)| key.to_unicode())
                });
                let modifiers = key_modifiers(state);
                let key_input = translate_key(key, unshifted).map(|key| KeyInput {
                    code,
                    key,
                    modifiers,
                    unshifted,
                    action: if view.pressed_keys.borrow().contains_key(&code) {
                        TerminalKeyAction::Repeat
                    } else {
                        TerminalKeyAction::Press
                    },
                    consumed_modifiers: event
                        .as_ref()
                        .map(|event| key_modifiers(event.consumed_modifiers() & state))
                        .unwrap_or_default(),
                });

                // A native IME may start composition on an ordinary key. Its
                // synchronous single-character commits keep the key metadata.
                let composing = !view.preedit.borrow().is_empty();
                if (composing || !(modifiers.control || modifiers.alt || modifiers.super_key))
                    && let Some(event) = event
                {
                    view.ime_key.set(
                        key_input.filter(|key| {
                            !composing && matches!(key.key, TerminalKey::Character(_))
                        }),
                    );
                    let consumed = input.filter_keypress(&event);
                    view.ime_key.set(None);
                    if consumed {
                        return glib::Propagation::Stop;
                    }
                }

                let mut utf8 = [0; 4];
                let text = key
                    .to_unicode()
                    .filter(|ch| !ch.is_control() && !modifiers.control)
                    .map(|ch| &*ch.encode_utf8(&mut utf8))
                    .unwrap_or_default();
                if key_input.is_some_and(|key| {
                    let text = if matches!(key.key, TerminalKey::Character(_)) {
                        text
                    } else {
                        ""
                    };
                    view.send_key(key, text)
                }) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        });
        keys.connect_key_released({
            let view = Rc::downgrade(&self.0);
            let input = input.clone();
            move |controller, _, code, state| {
                let Some(view) = view.upgrade() else { return };
                let pressed = view.pressed_keys.borrow_mut().remove(&code);
                if let Some((key, unshifted)) = pressed {
                    view.send_key(
                        KeyInput {
                            code,
                            key,
                            unshifted,
                            action: TerminalKeyAction::Release,
                            modifiers: key_modifiers(state),
                            consumed_modifiers: TerminalKeyModifiers::default(),
                        },
                        "",
                    );
                } else if let Some(event) = controller.current_event() {
                    input.filter_keypress(&event);
                }
            }
        });
        self.0.canvas.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter({
            let view = Rc::downgrade(&self.0);
            let input = input.clone();
            move |_| {
                input.focus_in();
                if let Some(view) = view.upgrade() {
                    view.focus(true);
                }
            }
        });
        focus.connect_leave({
            let view = Rc::downgrade(&self.0);
            move |_| {
                input.focus_out();
                if let Some(view) = view.upgrade() {
                    view.pressed_keys.borrow_mut().clear();
                    view.focus(false);
                }
            }
        });
        self.0.canvas.add_controller(focus);

        let click = gtk::GestureClick::builder().button(0).build();
        click.connect_pressed({
            let view = Rc::downgrade(&self.0);
            move |gesture, count, x, y| {
                let Some(view) = view.upgrade() else { return };
                view.canvas.grab_focus();
                let button = mouse_button(gesture.current_button());
                let state = gesture.current_event_state();
                let (line, col, side) = view.grid_point(x, y);
                let mut primary_paste = false;
                let activate_link = button == TerminalMouseButton::Left
                    && count == 1
                    && state.contains(gdk::ModifierType::CONTROL_MASK)
                    && !state.contains(gdk::ModifierType::ALT_MASK);
                let target = activate_link
                    .then(|| {
                        view.session
                            .borrow()
                            .as_ref()
                            .and_then(|session| session.link_at(line, col))
                    })
                    .flatten();
                if let Some(target) = target {
                    let activation = match target {
                        TerminalLink::Url(url) => TerminalActivation::Url(url),
                        TerminalLink::File { path, line, column } => {
                            TerminalActivation::File(TerminalFileActivation {
                                path,
                                line,
                                column,
                                launch_dir: view.launch_dir.borrow().clone(),
                            })
                        }
                    };
                    let handlers = view.activation_handlers.borrow().clone();
                    for handler in handlers {
                        handler(activation.clone());
                    }
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    return;
                }
                {
                    let session = view.session.borrow();
                    let Some(session) = session.as_ref() else {
                        return;
                    };
                    if view.report_mouse(session, button, TerminalMouseAction::Press, state, x, y) {
                        view.mouse_button.set(button);
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        return;
                    }
                    if button == TerminalMouseButton::Left {
                        let selection = match count {
                            2 => TerminalSelectionType::Semantic,
                            3.. => TerminalSelectionType::Lines,
                            _ if state.contains(gdk::ModifierType::ALT_MASK) => {
                                TerminalSelectionType::Block
                            }
                            _ => TerminalSelectionType::Simple,
                        };
                        session.begin_selection(line, col, selection, side);
                        view.selecting.set(true);
                    } else if button == TerminalMouseButton::Middle {
                        primary_paste = true;
                    }
                }
                if primary_paste {
                    GtkTerminal(view.clone()).paste_from(&view.canvas.primary_clipboard());
                }
                view.canvas.queue_draw();
            }
        });
        click.connect_released({
            let view = Rc::downgrade(&self.0);
            move |gesture, _, x, y| {
                let Some(view) = view.upgrade() else { return };
                let session = view.session.borrow();
                let Some(session) = session.as_ref() else {
                    return;
                };
                let button = view.mouse_button.replace(TerminalMouseButton::None);
                if button != TerminalMouseButton::None {
                    view.report_mouse(
                        session,
                        button,
                        TerminalMouseAction::Release,
                        gesture.current_event_state(),
                        x,
                        y,
                    );
                    return;
                }
                if view.selecting.replace(false)
                    && let Some(text) = session.selected_text()
                {
                    view.canvas.primary_clipboard().set_text(&text);
                }
            }
        });
        click.connect_stopped({
            let view = Rc::downgrade(&self.0);
            move |_| {
                if let Some(view) = view.upgrade() {
                    view.selecting.set(false);
                }
            }
        });
        self.0.canvas.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let view = Rc::downgrade(&self.0);
            move |_, x, y| {
                if let Some(view) = view.upgrade() {
                    view.pointer.set((x, y));
                }
            }
        });
        motion.connect_motion({
            let view = Rc::downgrade(&self.0);
            move |controller, x, y| {
                let Some(view) = view.upgrade() else { return };
                view.pointer.set((x, y));
                let session = view.session.borrow();
                let Some(session) = session.as_ref() else {
                    return;
                };
                if view.selecting.get() {
                    let (line, col, side) = view.grid_point(x, y);
                    session.update_selection(line, col, side);
                    view.canvas.queue_draw();
                } else {
                    view.report_mouse(
                        session,
                        view.mouse_button.get(),
                        TerminalMouseAction::Move,
                        controller.current_event_state(),
                        x,
                        y,
                    );
                }
            }
        });
        self.0.canvas.add_controller(motion);
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll({
            let view = Rc::downgrade(&self.0);
            move |controller, _, dy| {
                let Some(view) = view.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let state = controller.current_event_state();
                {
                    let session = view.session.borrow();
                    let Some(session) = session.as_ref() else {
                        return glib::Propagation::Proceed;
                    };
                    let (x, y) = view.pointer.get();
                    let button = if dy < 0.0 {
                        TerminalMouseButton::WheelUp
                    } else {
                        TerminalMouseButton::WheelDown
                    };
                    for _ in 0..(dy.abs().ceil() as usize).min(20) {
                        if !view.report_mouse(
                            session,
                            button,
                            TerminalMouseAction::Press,
                            state,
                            x,
                            y,
                        ) {
                            session.scroll(TerminalScroll::Lines((-dy * 3.0) as i32));
                            break;
                        }
                    }
                }
                view.refresh();
                glib::Propagation::Stop
            }
        });
        self.0.canvas.add_controller(scroll);
    }

    fn install_autoscroll(&self) {
        let marker = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();
        self.0.root.add_overlay(&marker);
        let autoscroll = Rc::new(canvas_scroll::MiddleAutoscroll::new());
        let axes = canvas_scroll::AutoscrollAxes::Vertical;
        canvas_scroll::install_middle_autoscroll_marker(&marker, &autoscroll, axes);
        let view = Rc::downgrade(&self.0);
        canvas_scroll::install_middle_autoscroll(
            &self.0.canvas,
            &autoscroll,
            axes,
            "terminal",
            {
                let view = view.clone();
                move || {
                    view.upgrade().is_some_and(|view| {
                        view.session.borrow().as_ref().is_some_and(|session| {
                            let (_, len, total) = session.scrollbar();
                            total > len && !session.mouse_reporting_enabled()
                        })
                    })
                }
            },
            {
                let view = view.clone();
                move |state| {
                    if let Some(view) = view.upgrade() {
                        let offset = canvas_scroll::middle_autoscroll_delta(
                            state.pointer.y - state.origin.y,
                        );
                        view.adjustment.set_value(
                            view.adjustment.value()
                                + offset / f64::from(view.viewport.get().cell_height),
                        );
                    }
                }
            },
            || {},
            || {},
            {
                let view = view.clone();
                move |cursor| {
                    if let Some(view) = view.upgrade() {
                        view.canvas.set_cursor_from_name(cursor);
                    }
                }
            },
            {
                let marker = marker.downgrade();
                move || {
                    if let Some(marker) = marker.upgrade() {
                        marker.queue_draw();
                    }
                }
            },
        );
    }

    fn install_context_menu(&self) {
        #[derive(Clone, Copy, Debug)]
        enum Action {
            Copy,
            CopyScreen,
            CopyAll,
            SelectAll,
            Paste,
        }

        let click = gtk::GestureClick::builder().button(3).build();
        click.connect_pressed({
            let weak = Rc::downgrade(&self.0);
            move |gesture, _, x, y| {
                let Some(view) = weak.upgrade() else { return };
                if !gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::SHIFT_MASK)
                    && view
                        .session
                        .borrow()
                        .as_ref()
                        .is_some_and(TerminalSession::mouse_reporting_enabled)
                {
                    return;
                }
                let terminal = GtkTerminal(view.clone());
                let popover = context_menu::popup_action_menu(
                    &view.canvas,
                    x,
                    y,
                    vec![
                        context_menu::ActionMenuSection::new(vec![
                            context_menu::ActionMenuItem::new(
                                "Copy",
                                Action::Copy,
                                terminal.has_selection(),
                            ),
                            context_menu::ActionMenuItem::new(
                                "Copy Screen",
                                Action::CopyScreen,
                                true,
                            ),
                            context_menu::ActionMenuItem::new("Copy All", Action::CopyAll, true),
                        ]),
                        context_menu::ActionMenuSection::new(vec![
                            context_menu::ActionMenuItem::new(
                                "Select All",
                                Action::SelectAll,
                                true,
                            ),
                            context_menu::ActionMenuItem::new("Paste", Action::Paste, true),
                        ]),
                    ],
                    {
                        let weak = weak.clone();
                        move |action| {
                            let Some(view) = weak.upgrade() else { return };
                            let terminal = GtkTerminal(view.clone());
                            match action {
                                Action::Copy => terminal.copy_clipboard(),
                                Action::CopyScreen | Action::CopyAll => {
                                    if let Some(session) = view.session.borrow().as_ref() {
                                        let text = match action {
                                            Action::CopyScreen => session.visible_text(),
                                            _ => session.all_text(),
                                        };
                                        view.canvas.clipboard().set_text(&text);
                                    }
                                }
                                Action::SelectAll => {
                                    if let Some(session) = view.session.borrow().as_ref() {
                                        session.select_all();
                                        if let Some(text) = session.selected_text() {
                                            view.canvas.primary_clipboard().set_text(&text);
                                        }
                                    }
                                    view.canvas.queue_draw();
                                }
                                Action::Paste => terminal.paste_clipboard(),
                            }
                            terminal.grab_focus();
                        }
                    },
                );
                popover.connect_closed(|popover| {
                    let popover = popover.clone();
                    glib::idle_add_local_once(move || popover.unparent());
                });
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.0.canvas.add_controller(click);
    }

    fn install_file_drop(&self) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop({
            let view = Rc::downgrade(&self.0);
            move |_, value, _, _| {
                let (Some(view), Ok(files)) = (view.upgrade(), value.get::<gdk::FileList>()) else {
                    return false;
                };
                let paths: Vec<_> = files.files().iter().filter_map(gio::File::path).collect();
                if paths.is_empty() {
                    return false;
                }
                let handler = view.file_drop_handler.borrow().clone();
                let terminal = GtkTerminal(view);
                if let Some(handler) = handler {
                    handler(terminal, paths);
                } else {
                    terminal.paste_file_paths(&paths);
                }
                true
            }
        });
        self.0.canvas.add_controller(target);
    }
}

impl View {
    fn resize(&self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        let (cell_width, cell_height) = self.paint.borrow_mut().cell_size(self.font_size.get());
        let viewport = TerminalViewport::new(
            width as usize / usize::from(cell_width),
            height as usize / usize::from(cell_height),
            cell_width,
            cell_height,
        );
        if self.viewport.replace(viewport) != viewport {
            if let Some(session) = self.session.borrow_mut().as_mut()
                && let Err(error) = session.resize(viewport)
            {
                log::warn!("terminal resize failed: {error}");
            }
            self.refresh();
        }
    }

    fn refresh(&self) {
        let scrollbar = self
            .session
            .borrow()
            .as_ref()
            .map(TerminalSession::scrollbar);
        if let Some((offset, rows, total)) = scrollbar {
            self.adjusting.set(true);
            self.adjustment.configure(
                offset as f64,
                0.0,
                total as f64,
                1.0,
                rows as f64,
                rows as f64,
            );
            self.adjusting.set(false);
        }
        self.canvas.queue_draw();
    }

    fn drain_events(&self) -> Option<Duration> {
        let batch = self
            .session
            .borrow()
            .as_ref()
            .map(TerminalSession::drain_events)?;
        if let Some((kind, text)) = batch.clipboard_store {
            match kind {
                TerminalClipboard::Clipboard => self.canvas.clipboard(),
                TerminalClipboard::Selection => self.canvas.primary_clipboard(),
            }
            .set_text(&text);
        }
        if let Some(active) = batch.activity {
            let handlers = self.activity_handlers.borrow().clone();
            for handler in handlers {
                handler(active);
            }
        }
        if let Some(title) = batch.title {
            self.title.replace(title.clone());
            let title = title.unwrap_or_default();
            let handlers = self.title_handlers.borrow().clone();
            for handler in handlers {
                handler(title.clone());
            }
        }
        if batch.bell {
            self.canvas.error_bell();
        }
        if batch.needs_redraw {
            self.refresh();
        }
        if batch.exited {
            let handlers = self.exit_handlers.take();
            let status = batch
                .exit_status
                .unwrap_or_else(|| ExitStatus::from_raw(1 << 8));
            for handler in handlers {
                handler(status);
            }
        }
        batch.next_animation
    }

    fn send_key(&self, input: KeyInput, text: &str) -> bool {
        let sent = {
            let session = self.session.borrow();
            let Some(session) = session
                .as_ref()
                .filter(|session| session.lifecycle() == TerminalLifecycle::Running)
            else {
                return false;
            };
            session
                .send_key(TerminalKeyEvent {
                    key: input.key,
                    action: input.action,
                    modifiers: input.modifiers,
                    consumed_modifiers: input.consumed_modifiers,
                    text,
                    unshifted: input.unshifted,
                })
                .unwrap_or_else(|error| {
                    log::debug!("terminal key input failed: {error}");
                    false
                })
        };
        if sent {
            if input.action != TerminalKeyAction::Release {
                self.pressed_keys
                    .borrow_mut()
                    .insert(input.code, (input.key, input.unshifted));
            }
            self.refresh();
        }
        sent
    }

    fn focus(&self, focused: bool) {
        if let Some(session) = self.session.borrow().as_ref() {
            session.report_focus(focused);
        }
        self.canvas.queue_draw();
    }

    fn grid_point(&self, x: f64, y: f64) -> (usize, usize, TerminalSide) {
        let viewport = self.viewport.get();
        let width = f64::from(viewport.cell_width);
        let x = x.max(0.0);
        (
            (y.max(0.0) / f64::from(viewport.cell_height)) as usize,
            (x / width) as usize,
            if x % width < width / 2.0 {
                TerminalSide::Left
            } else {
                TerminalSide::Right
            },
        )
    }

    fn report_mouse(
        &self,
        session: &TerminalSession,
        button: TerminalMouseButton,
        action: TerminalMouseAction,
        state: gdk::ModifierType,
        x: f64,
        y: f64,
    ) -> bool {
        let keys = key_modifiers(state);
        !keys.shift
            && session.mouse_reporting_enabled()
            && session.report_mouse_at(
                button,
                action,
                TerminalMouseModifiers {
                    shift: keys.shift,
                    option: keys.alt,
                    control: keys.control,
                },
                x as f32,
                y as f32,
            )
    }
}

fn key_modifiers(state: gdk::ModifierType) -> TerminalKeyModifiers {
    TerminalKeyModifiers {
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        alt: state.contains(gdk::ModifierType::ALT_MASK),
        control: state.contains(gdk::ModifierType::CONTROL_MASK),
        super_key: state.intersects(gdk::ModifierType::SUPER_MASK | gdk::ModifierType::META_MASK),
        caps_lock: state.contains(gdk::ModifierType::LOCK_MASK),
        num_lock: false,
    }
}

fn translate_key(key: gdk::Key, unshifted: Option<char>) -> Option<TerminalKey> {
    use TerminalKey::*;
    use gdk::Key;
    Some(match key {
        Key::Return => Enter,
        Key::KP_Enter => KeypadEnter,
        Key::Escape => Escape,
        Key::Tab | Key::ISO_Left_Tab | Key::KP_Tab => Tab,
        Key::BackSpace => Backspace,
        Key::Delete => Delete,
        Key::Insert => Insert,
        Key::Home => Home,
        Key::End => End,
        Key::Page_Up => PageUp,
        Key::Page_Down => PageDown,
        Key::Up => Up,
        Key::Down => Down,
        Key::Left => Left,
        Key::Right => Right,
        Key::KP_0 | Key::KP_Insert => Keypad(0),
        Key::KP_1 | Key::KP_End => Keypad(1),
        Key::KP_2 | Key::KP_Down => Keypad(2),
        Key::KP_3 | Key::KP_Page_Down => Keypad(3),
        Key::KP_4 | Key::KP_Left => Keypad(4),
        Key::KP_5 | Key::KP_Begin => Keypad(5),
        Key::KP_6 | Key::KP_Right => Keypad(6),
        Key::KP_7 | Key::KP_Home => Keypad(7),
        Key::KP_8 | Key::KP_Up => Keypad(8),
        Key::KP_9 | Key::KP_Page_Up => Keypad(9),
        Key::KP_Add => KeypadAdd,
        Key::KP_Subtract => KeypadSubtract,
        Key::KP_Multiply => KeypadMultiply,
        Key::KP_Divide => KeypadDivide,
        Key::KP_Decimal | Key::KP_Delete => KeypadDecimal,
        Key::KP_Equal => KeypadEqual,
        _ if (Key::F1..=Key::F25).contains(&key) => {
            Function((key.into_glib() - Key::F1.into_glib() + 1) as u8)
        }
        _ => Character(unshifted.or_else(|| key.to_unicode())?),
    })
}

fn mouse_button(button: u32) -> TerminalMouseButton {
    match button {
        1 => TerminalMouseButton::Left,
        2 => TerminalMouseButton::Middle,
        3 => TerminalMouseButton::Right,
        _ => TerminalMouseButton::None,
    }
}
