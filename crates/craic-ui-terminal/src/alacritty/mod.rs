mod renderer;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config as TermConfig, TermMode};
use alacritty_terminal::tty::{self, Options, Shell};
use alacritty_terminal::vte::ansi::Processor;
use craic_ui_core::ui::{canvas_scroll, components::context_menu};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use renderer::{GlRenderer, TerminalSize};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::ui::components::terminal::{TerminalActivation, TerminalFileActivation};

const INITIAL_CELL_WIDTH: f32 = 9.0;
const INITIAL_CELL_HEIGHT: f32 = 18.0;
const INITIAL_COLUMNS: u32 = 100;
const INITIAL_ROWS: u32 = 34;
const SCROLLBACK_LINES: usize = 10_000;
const SCROLL_LINES_PER_WHEEL_STEP: i32 = 3;

type ExitHandlers = Rc<RefCell<Vec<Box<dyn Fn(ExitStatus)>>>>;
type TitleHandlers = Rc<RefCell<Vec<Box<dyn Fn(String)>>>>;
type ActivationHandlers = Rc<RefCell<Vec<Box<dyn Fn(TerminalActivation)>>>>;

#[derive(Clone, Copy)]
enum TerminalContextAction {
    Copy,
    CopyScreen,
    CopyAll,
    SelectAll,
    Paste,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LinkRange {
    start: Point,
    end: Point,
}

impl LinkRange {
    pub(super) fn contains(self, point: Point) -> bool {
        point >= self.start && point <= self.end
    }
}

#[derive(Clone)]
pub struct AlacrittyTerminal {
    root: gtk::Overlay,
    area: gtk::GLArea,
    state: Rc<RefCell<UiState>>,
}

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub env: HashMap<String, String>,
}

pub fn terminal_environment() -> HashMap<String, String> {
    let mut environment = std::env::vars().collect::<HashMap<_, _>>();
    environment.insert("TERM".to_string(), "xterm-256color".to_string());
    environment.insert("COLORTERM".to_string(), "truecolor".to_string());
    environment.insert("TERM_PROGRAM".to_string(), "Craic".to_string());
    if !["LC_ALL", "LC_CTYPE", "LANG"].into_iter().any(|key| {
        environment.get(key).is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
    }) {
        environment.insert("LC_CTYPE".to_string(), "C.UTF-8".to_string());
    }
    environment
}

struct UiState {
    engine: Option<TerminalEngine>,
    renderer: Option<GlRenderer>,
    receiver: Receiver<Event>,
    proxy: TerminalEventProxy,
    font_size: f64,
    renderer_font_size: f64,
    focused: bool,
    title: Arc<Mutex<Option<String>>>,
    exit_handlers: ExitHandlers,
    title_handlers: TitleHandlers,
    activation_handlers: ActivationHandlers,
    launch_dir: String,
    mouse_buttons: u8,
    last_mouse_point: Option<Point>,
    hovered_link: Option<LinkRange>,
    scroll_remainder: f64,
    scrollbar_adjustment: gtk::Adjustment,
    syncing_scrollbar: bool,
    exited: bool,
}

struct TerminalEngine {
    term: Arc<FairMutex<Term<TerminalEventProxy>>>,
    notifier: Notifier,
    parser: Processor,
    child_pid: i32,
    pty_file: File,
}

#[derive(Clone)]
pub(super) struct TerminalEventProxy {
    sender: Sender<Event>,
    dirty: Arc<AtomicBool>,
    title: Arc<Mutex<Option<String>>>,
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        match &event {
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {
                self.dirty.store(true, Ordering::Release);
            }
            Event::Title(title) => {
                if let Ok(mut current) = self.title.lock() {
                    *current = Some(title.clone());
                }
                let _ = self.sender.send(event);
                self.dirty.store(true, Ordering::Release);
            }
            Event::ResetTitle => {
                if let Ok(mut current) = self.title.lock() {
                    *current = None;
                }
                let _ = self.sender.send(event);
                self.dirty.store(true, Ordering::Release);
            }
            _ => {
                let _ = self.sender.send(event);
                self.dirty.store(true, Ordering::Release);
            }
        }
    }
}

impl TerminalEngine {
    fn spawn(spec: SpawnSpec, proxy: TerminalEventProxy) -> Result<Self, String> {
        let initial_size = TerminalSize {
            width: (INITIAL_COLUMNS as f32 * INITIAL_CELL_WIDTH) as u32,
            height: (INITIAL_ROWS as f32 * INITIAL_CELL_HEIGHT) as u32,
            cell_width: INITIAL_CELL_WIDTH,
            cell_height: INITIAL_CELL_HEIGHT,
        };
        let config = TermConfig {
            scrolling_history: SCROLLBACK_LINES,
            ..TermConfig::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &initial_size,
            proxy.clone(),
        )));
        let options = Options {
            shell: Some(Shell::new(spec.program, spec.args)),
            working_directory: Some(spec.working_directory),
            drain_on_exit: true,
            env: spec.env,
            #[cfg(target_os = "windows")]
            escape_args: true,
        };
        let pty = tty::new(&options, initial_size.into(), 0).map_err(|err| err.to_string())?;
        let child_pid = pty.child().id() as i32;
        let pty_file = pty.file().try_clone().map_err(|err| err.to_string())?;
        let event_loop =
            EventLoop::new(term.clone(), proxy, pty, true, false).map_err(|err| err.to_string())?;
        let notifier = Notifier(event_loop.channel());
        let _pty_thread = event_loop.spawn();
        log::info!("alacritty terminal PTY started pid={child_pid}");

        Ok(Self {
            term,
            notifier,
            parser: Processor::new(),
            child_pid,
            pty_file,
        })
    }

    fn input<B: Into<Cow<'static, [u8]>>>(&self, bytes: B) {
        self.notifier.notify(bytes);
    }

    fn resize(&mut self, size: TerminalSize) {
        self.notifier.on_resize(size.into());
        self.term.lock().resize(size);
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut *self.term.lock(), bytes);
    }

    fn paste(&self, text: String) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            self.input(&b"\x1b[200~"[..]);
            self.input(text.replace('\x1b', "").into_bytes());
            self.input(&b"\x1b[201~"[..]);
        } else {
            self.input(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
        }
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    fn has_selection(&self) -> bool {
        self.term.lock().selection_to_string().is_some()
    }

    fn visible_text(&self) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        let start_line = -(grid.display_offset() as i32);
        let end_line = start_line + grid.screen_lines().saturating_sub(1) as i32;
        term.bounds_to_string(
            Point::new(Line(start_line), Column(0)),
            Point::new(Line(end_line), Column(grid.columns().saturating_sub(1))),
        )
    }

    fn all_text(&self) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        term.bounds_to_string(
            Point::new(Line(-(grid.history_size() as i32)), Column(0)),
            Point::new(
                Line(grid.screen_lines().saturating_sub(1) as i32),
                Column(grid.columns().saturating_sub(1)),
            ),
        )
    }

    fn select_all(&self) {
        let mut term = self.term.lock();
        let grid = term.grid();
        let start = Point::new(Line(-(grid.history_size() as i32)), Column(0));
        let end = Point::new(
            Line(grid.screen_lines().saturating_sub(1) as i32),
            Column(grid.columns().saturating_sub(1)),
        );
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
    }

    fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    fn mouse_mode(&self) -> bool {
        self.term.lock().mode().intersects(TermMode::MOUSE_MODE)
    }

    fn mouse_report(&self, button: u8, pressed: bool, point: Point, modifiers: gdk::ModifierType) {
        if point.line.0 < 0 {
            return;
        }
        let mode = *self.term.lock().mode();
        let mut modifier_code = 0;
        if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
            modifier_code += 4;
        }
        if modifiers.contains(gdk::ModifierType::ALT_MASK) {
            modifier_code += 8;
        }
        if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            modifier_code += 16;
        }
        let button = button + modifier_code;
        if mode.contains(TermMode::SGR_MOUSE) {
            let suffix = if pressed { 'M' } else { 'm' };
            self.input(
                format!(
                    "\x1b[<{button};{};{}{suffix}",
                    point.column.0 + 1,
                    point.line.0 + 1
                )
                .into_bytes(),
            );
            return;
        }

        let button = if pressed { button } else { 3 + modifier_code };
        if point.column.0 >= 223 || point.line.0 >= 223 {
            return;
        }
        self.input(vec![
            b'\x1b',
            b'[',
            b'M',
            32 + button,
            33 + point.column.0 as u8,
            33 + point.line.0 as u8,
        ]);
    }

    fn foreground_process_group(&self) -> Option<libc::pid_t> {
        let pgid = unsafe { libc::tcgetpgrp(self.pty_file.as_raw_fd()) };
        (pgid > 0).then_some(pgid)
    }
}

impl Drop for TerminalEngine {
    fn drop(&mut self) {
        if let Err(err) = self.notifier.0.send(Msg::Shutdown) {
            log::debug!("alacritty terminal PTY shutdown already complete: {err}");
        } else {
            log::info!(
                "alacritty terminal PTY shutdown requested pid={}",
                self.child_pid
            );
        }
    }
}

impl AlacrittyTerminal {
    pub fn new(font_size: f64) -> Self {
        let area = gtk::GLArea::builder()
            .auto_render(false)
            .focusable(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        area.set_required_version(3, 0);
        let scrollbar_adjustment = gtk::Adjustment::new(0.0, 0.0, 1.0, 1.0, 1.0, 1.0);
        let scrollbar =
            gtk::Scrollbar::new(gtk::Orientation::Vertical, Some(&scrollbar_adjustment));
        scrollbar.set_focusable(false);
        scrollbar.set_halign(gtk::Align::End);
        scrollbar.set_valign(gtk::Align::Fill);
        scrollbar.add_css_class("overlay-indicator");
        scrollbar.add_css_class("right");
        scrollbar.set_visible(false);
        let scrollbar_hover = gtk::EventControllerMotion::new();
        scrollbar_hover.connect_enter({
            let scrollbar = scrollbar.clone();
            move |_, _, _| scrollbar.add_css_class("hovering")
        });
        scrollbar_hover.connect_leave({
            let scrollbar = scrollbar.clone();
            move |_| scrollbar.remove_css_class("hovering")
        });
        scrollbar.add_controller(scrollbar_hover);

        let root = gtk::Overlay::new();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_child(Some(&area));
        let autoscroll_marker = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();
        root.add_overlay(&autoscroll_marker);
        root.add_overlay(&scrollbar);

        let (sender, receiver) = mpsc::channel();
        let title = Arc::new(Mutex::new(None));
        let proxy = TerminalEventProxy {
            sender,
            dirty: Arc::new(AtomicBool::new(true)),
            title: title.clone(),
        };
        let state = Rc::new(RefCell::new(UiState {
            engine: None,
            renderer: None,
            receiver,
            proxy,
            font_size,
            renderer_font_size: font_size,
            focused: false,
            title,
            exit_handlers: Rc::new(RefCell::new(Vec::new())),
            title_handlers: Rc::new(RefCell::new(Vec::new())),
            activation_handlers: Rc::new(RefCell::new(Vec::new())),
            launch_dir: String::new(),
            mouse_buttons: 0,
            last_mouse_point: None,
            hovered_link: None,
            scroll_remainder: 0.0,
            scrollbar_adjustment: scrollbar_adjustment.clone(),
            syncing_scrollbar: false,
            exited: false,
        }));

        install_gl_lifecycle(&area, &state);
        install_input(&area, &state);
        install_selection(&area, &state);
        install_scroll(&area, &state);
        install_scrollbar(&area, &scrollbar_adjustment, &state);
        install_middle_autoscroll(&area, &autoscroll_marker, &state);
        install_focus(&area, &state);
        install_context_menu(&area, &state);
        install_file_drop(&area, &state);

        area.add_tick_callback({
            let state = state.clone();
            let scrollbar = scrollbar.clone();
            move |area, _| {
                drain_events(area, &state);
                sync_scrollbar(&state, &scrollbar);
                if state.borrow().proxy.dirty.swap(false, Ordering::AcqRel) {
                    area.queue_render();
                }
                glib::ControlFlow::Continue
            }
        });

        Self { root, area, state }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub fn area(&self) -> &gtk::GLArea {
        &self.area
    }

    pub fn grab_focus(&self) {
        self.area.grab_focus();
    }

    pub fn spawn(&self, spec: SpawnSpec, launch_dir: String) -> Result<i32, String> {
        let mut state = self.state.borrow_mut();
        if state.engine.is_some() {
            return Err("Terminal process has already been started.".to_string());
        }
        let mut engine = TerminalEngine::spawn(spec, state.proxy.clone())?;
        let child_pid = engine.child_pid;
        engine.term.lock().is_focused = state.focused;
        if let Some(size) = state.renderer.as_ref().map(GlRenderer::size) {
            engine.resize(size);
        }
        state.launch_dir = launch_dir;
        state.engine = Some(engine);
        state.proxy.dirty.store(true, Ordering::Release);
        Ok(child_pid)
    }

    pub fn connect_child_exited<F: Fn(ExitStatus) + 'static>(&self, callback: F) {
        self.state
            .borrow()
            .exit_handlers
            .borrow_mut()
            .push(Box::new(callback));
    }

    pub fn connect_title_changed<F: Fn(String) + 'static>(&self, callback: F) {
        self.state
            .borrow()
            .title_handlers
            .borrow_mut()
            .push(Box::new(callback));
    }

    pub fn connect_activation<F: Fn(TerminalActivation) + 'static>(&self, callback: F) {
        self.state
            .borrow()
            .activation_handlers
            .borrow_mut()
            .push(Box::new(callback));
    }

    pub fn title(&self) -> Option<String> {
        self.state
            .borrow()
            .title
            .lock()
            .ok()
            .and_then(|title| title.clone())
    }

    pub fn foreground_process_group(&self) -> Option<libc::pid_t> {
        self.state
            .borrow()
            .engine
            .as_ref()
            .and_then(TerminalEngine::foreground_process_group)
    }

    pub fn set_font_size(&self, font_size: f64) {
        self.state.borrow_mut().font_size = font_size;
        self.state
            .borrow()
            .proxy
            .dirty
            .store(true, Ordering::Release);
    }

    pub fn has_selection(&self) -> bool {
        self.state
            .borrow()
            .engine
            .as_ref()
            .is_some_and(TerminalEngine::has_selection)
    }

    pub fn copy_clipboard(&self) {
        let Some(text) = self
            .state
            .borrow()
            .engine
            .as_ref()
            .and_then(TerminalEngine::selection_text)
        else {
            return;
        };
        self.area.clipboard().set_text(&text);
    }

    pub fn paste_clipboard(&self) {
        paste_clipboard(&self.area, &self.state);
    }

    pub fn paste_text(&self, text: &str) {
        if let Some(engine) = self.state.borrow().engine.as_ref() {
            engine.paste(text.to_string());
        }
    }

    pub fn feed_child(&self, bytes: &[u8]) {
        if let Some(engine) = self.state.borrow().engine.as_ref() {
            engine.input(bytes.to_vec());
        }
    }

    pub fn feed(&self, bytes: &[u8]) {
        let mut state = self.state.borrow_mut();
        let Some(engine) = state.engine.as_mut() else {
            return;
        };
        engine.feed(bytes);
        state.proxy.dirty.store(true, Ordering::Release);
    }

    pub fn search(&self, pattern: Option<&str>, backwards: bool) -> Result<bool, String> {
        let (term, dirty) = {
            let state = self.state.borrow();
            let Some(engine) = state.engine.as_ref() else {
                return Ok(false);
            };
            (engine.term.clone(), state.proxy.dirty.clone())
        };
        let mut term = term.lock();
        let Some(pattern) = pattern else {
            term.selection = None;
            dirty.store(true, Ordering::Release);
            return Ok(false);
        };
        let mut regex = RegexSearch::new(pattern).map_err(|err| err.to_string())?;
        let grid = term.grid();
        let origin = term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&term))
            .map(|range| {
                if backwards {
                    range.start.sub(grid, Boundary::Grid, 1)
                } else {
                    range.end.add(grid, Boundary::Grid, 1)
                }
            })
            .unwrap_or_else(|| {
                if backwards {
                    Point::new(
                        Line(grid.screen_lines() as i32 - 1),
                        Column(grid.columns() - 1),
                    )
                } else {
                    Point::new(Line(-(grid.history_size() as i32)), Column(0))
                }
            });
        let direction = if backwards {
            Direction::Left
        } else {
            Direction::Right
        };
        let side = if backwards { Side::Right } else { Side::Left };
        let Some(found) = term.search_next(&mut regex, origin, direction, side, None) else {
            return Ok(false);
        };
        if backwards {
            term.scroll_to_point(*found.start());
        } else {
            term.scroll_to_point(*found.end());
        }
        let mut selection = Selection::new(SelectionType::Simple, *found.start(), Side::Left);
        selection.update(*found.end(), Side::Right);
        term.selection = Some(selection);
        dirty.store(true, Ordering::Release);
        Ok(true)
    }

    pub fn all_text(&self) -> Option<String> {
        let state = self.state.borrow();
        let engine = state.engine.as_ref()?;
        Some(engine.all_text())
    }

    pub fn cursor_row(&self) -> Option<i64> {
        let state = self.state.borrow();
        let engine = state.engine.as_ref()?;
        let term = engine.term.lock();
        Some(term.grid().history_size() as i64 + term.grid().cursor.point.line.0 as i64)
    }

    pub fn text_before_cursor(&self, max_lines: usize) -> Option<String> {
        let state = self.state.borrow();
        let engine = state.engine.as_ref()?;
        let term = engine.term.lock();
        let grid = term.grid();
        let end_line = term.grid().cursor.point.line.0 - 1;
        let first_line = -(grid.history_size() as i32);
        if end_line < first_line {
            return None;
        }
        let start_line = (end_line - max_lines.saturating_sub(1) as i32).max(first_line);
        Some(term.bounds_to_string(
            Point::new(Line(start_line), Column(0)),
            Point::new(Line(end_line), Column(grid.columns().saturating_sub(1))),
        ))
    }

    pub fn recent_text(&self, max_lines: usize) -> Option<String> {
        let state = self.state.borrow();
        let engine = state.engine.as_ref()?;
        let term = engine.term.lock();
        let grid = term.grid();
        let cursor_end = grid.cursor.point.line.0.saturating_sub(1);
        let end_line = (grid.screen_lines().saturating_sub(1) as i32).max(cursor_end);
        let first_line = -(grid.history_size() as i32);
        let start_line = (end_line - max_lines.saturating_sub(1) as i32).max(first_line);
        Some(term.bounds_to_string(
            Point::new(Line(start_line), Column(0)),
            Point::new(Line(end_line), Column(grid.columns().saturating_sub(1))),
        ))
    }
}

fn install_gl_lifecycle(area: &gtk::GLArea, state: &Rc<RefCell<UiState>>) {
    area.connect_realize({
        let state = state.clone();
        move |area| {
            area.make_current();
            if let Some(err) = area.error() {
                log::error!("alacritty GL context realization failed: {err}");
                return;
            }
            let scale = area.scale_factor() as f32;
            let width = (area.width().max(1) as f32 * scale) as u32;
            let height = (area.height().max(1) as f32 * scale) as u32;
            let font_size = state.borrow().font_size;
            let uses_es = area.context().is_some_and(|context| context.uses_es());
            match GlRenderer::new(font_size, scale, width, height, uses_es) {
                Ok(renderer) => {
                    let size = renderer.size();
                    let mut state = state.borrow_mut();
                    state.renderer_font_size = font_size;
                    if let Some(engine) = state.engine.as_mut() {
                        engine.resize(size);
                    }
                    state.renderer = Some(renderer);
                    log::info!(
                        "alacritty GL terminal realized width={} height={} scale={scale}",
                        width,
                        height
                    );
                }
                Err(err) => {
                    log::error!("alacritty GL renderer initialization failed: {err}");
                    area.set_error(Some(&glib::Error::new(
                        gio::IOErrorEnum::Failed,
                        &format!("Unable to initialize terminal renderer: {err}"),
                    )));
                }
            }
        }
    });
    area.connect_resize({
        let state = state.clone();
        move |area, width, height| {
            let scale = area.scale_factor() as u32;
            let mut state = state.borrow_mut();
            let Some(renderer) = state.renderer.as_mut() else {
                return;
            };
            if renderer.resize(
                (width.max(1) as u32).saturating_mul(scale),
                (height.max(1) as u32).saturating_mul(scale),
            ) {
                let size = renderer.size();
                if let Some(engine) = state.engine.as_mut() {
                    engine.resize(size);
                }
                log::debug!(
                    "alacritty GL terminal resized columns={} rows={}",
                    size.columns(),
                    size.screen_lines()
                );
            }
        }
    });
    area.connect_render({
        let state = state.clone();
        move |area, _| {
            let mut state = state.borrow_mut();
            let scale = area.scale_factor() as f32;
            if state.renderer.as_ref().is_some_and(|renderer| {
                (renderer.scale() - scale).abs() > f32::EPSILON
                    || (state.renderer_font_size - state.font_size).abs() > f64::EPSILON
            }) {
                let width = (area.width().max(1) as f32 * scale) as u32;
                let height = (area.height().max(1) as f32 * scale) as u32;
                let font_size = state.font_size;
                let uses_es = area.context().is_some_and(|context| context.uses_es());
                state.renderer.take();
                match GlRenderer::new(font_size, scale, width, height, uses_es) {
                    Ok(renderer) => {
                        let size = renderer.size();
                        state.renderer_font_size = font_size;
                        if let Some(engine) = state.engine.as_mut() {
                            engine.resize(size);
                        }
                        state.renderer = Some(renderer);
                        log::info!(
                            "alacritty GL renderer rebuilt font_size={font_size} scale={scale}"
                        );
                    }
                    Err(err) => {
                        log::error!("alacritty GL renderer rebuild failed: {err}");
                    }
                }
            }
            let focused = state.focused;
            let hovered_link = state.hovered_link;
            let Some(engine) = state.engine.as_ref() else {
                return glib::Propagation::Proceed;
            };
            let term = engine.term.clone();
            let Some(renderer) = state.renderer.as_mut() else {
                return glib::Propagation::Proceed;
            };
            renderer.draw(&term.lock(), focused, hovered_link);
            glib::Propagation::Stop
        }
    });
    area.connect_unrealize({
        let state = state.clone();
        move |area| {
            area.make_current();
            state.borrow_mut().renderer.take();
            log::info!("alacritty GL terminal unrealized; GPU resources released");
        }
    });
}

fn install_input(area: &gtk::GLArea, state: &Rc<RefCell<UiState>>) {
    let keys = gtk::EventControllerKey::new();
    let im = gtk::IMMulticontext::new();
    keys.set_im_context(Some(&im));
    im.connect_commit({
        let state = state.clone();
        move |_, text| input_bytes(&state, text.as_bytes().to_vec())
    });
    keys.connect_key_pressed({
        let state = state.clone();
        move |_, key, _, modifiers| {
            let Some(bytes) = key_sequence(key, modifiers, &state) else {
                return glib::Propagation::Proceed;
            };
            input_bytes(&state, bytes);
            glib::Propagation::Stop
        }
    });
    area.add_controller(keys);
}

fn key_sequence(
    key: gdk::Key,
    modifiers: gdk::ModifierType,
    state: &Rc<RefCell<UiState>>,
) -> Option<Vec<u8>> {
    let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
    if ctrl && shift && matches!(key, gdk::Key::v | gdk::Key::V) {
        return None;
    }

    let app_cursor = state
        .borrow()
        .engine
        .as_ref()
        .is_some_and(|engine| engine.term.lock().mode().contains(TermMode::APP_CURSOR));
    let sequence = match key {
        gdk::Key::Return | gdk::Key::KP_Enter => Some(b"\r".to_vec()),
        gdk::Key::BackSpace => Some(vec![0x7f]),
        gdk::Key::Tab if shift => Some(b"\x1b[Z".to_vec()),
        gdk::Key::Tab => Some(b"\t".to_vec()),
        gdk::Key::Escape => Some(vec![0x1b]),
        gdk::Key::Up => Some(if app_cursor { b"\x1bOA" } else { b"\x1b[A" }.to_vec()),
        gdk::Key::Down => Some(if app_cursor { b"\x1bOB" } else { b"\x1b[B" }.to_vec()),
        gdk::Key::Right => Some(if app_cursor { b"\x1bOC" } else { b"\x1b[C" }.to_vec()),
        gdk::Key::Left => Some(if app_cursor { b"\x1bOD" } else { b"\x1b[D" }.to_vec()),
        gdk::Key::Home => Some(b"\x1b[H".to_vec()),
        gdk::Key::End => Some(b"\x1b[F".to_vec()),
        gdk::Key::Insert => Some(b"\x1b[2~".to_vec()),
        gdk::Key::Delete => Some(b"\x1b[3~".to_vec()),
        gdk::Key::Page_Up => Some(b"\x1b[5~".to_vec()),
        gdk::Key::Page_Down => Some(b"\x1b[6~".to_vec()),
        gdk::Key::F1 => Some(b"\x1bOP".to_vec()),
        gdk::Key::F2 => Some(b"\x1bOQ".to_vec()),
        gdk::Key::F3 => Some(b"\x1bOR".to_vec()),
        gdk::Key::F4 => Some(b"\x1bOS".to_vec()),
        gdk::Key::F5 => Some(b"\x1b[15~".to_vec()),
        gdk::Key::F6 => Some(b"\x1b[17~".to_vec()),
        gdk::Key::F7 => Some(b"\x1b[18~".to_vec()),
        gdk::Key::F8 => Some(b"\x1b[19~".to_vec()),
        gdk::Key::F9 => Some(b"\x1b[20~".to_vec()),
        gdk::Key::F10 => Some(b"\x1b[21~".to_vec()),
        gdk::Key::F11 => Some(b"\x1b[23~".to_vec()),
        gdk::Key::F12 => Some(b"\x1b[24~".to_vec()),
        _ => key.to_unicode().map(|character| {
            if ctrl {
                let lower = character.to_ascii_lowercase();
                match lower {
                    '@' | ' ' => vec![0],
                    'a'..='z' => vec![lower as u8 - b'a' + 1],
                    '[' => vec![0x1b],
                    '\\' => vec![0x1c],
                    ']' => vec![0x1d],
                    '^' => vec![0x1e],
                    '_' => vec![0x1f],
                    '?' => vec![0x7f],
                    _ => character.to_string().into_bytes(),
                }
            } else {
                character.to_string().into_bytes()
            }
        }),
    }?;
    if alt {
        let mut prefixed = Vec::with_capacity(sequence.len() + 1);
        prefixed.push(0x1b);
        prefixed.extend(sequence);
        Some(prefixed)
    } else {
        Some(sequence)
    }
}

fn input_bytes(state: &Rc<RefCell<UiState>>, bytes: Vec<u8>) {
    if let Some(engine) = state.borrow().engine.as_ref() {
        engine.input(bytes);
        engine.term.lock().scroll_display(Scroll::Bottom);
    }
}

fn install_selection(area: &gtk::GLArea, state: &Rc<RefCell<UiState>>) {
    let selection_drag_active = Rc::new(Cell::new(false));
    let drag_autoscroll_generation = Rc::new(Cell::new(0_u64));
    let drag_autoscroll_active = Rc::new(Cell::new(false));
    let drag_autoscroll_pointer = Rc::new(Cell::new(None::<(f64, f64)>));
    let click = gtk::GestureClick::builder().button(1).build();
    click.connect_pressed({
        let drag_autoscroll_generation = drag_autoscroll_generation.clone();
        let drag_autoscroll_active = drag_autoscroll_active.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        let state = state.clone();
        move |gesture, count, x, y| {
            if drag_autoscroll_active.replace(false) {
                drag_autoscroll_generation.set(drag_autoscroll_generation.get().wrapping_add(1));
            }
            drag_autoscroll_pointer.set(None);
            let Some(point) = point_at(&state, x, y) else {
                return;
            };
            let modifiers = gesture.current_event_state();
            if report_mouse_button(&state, point, 0, 1, true, modifiers) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }
            if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                activate_at(&state, point);
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }
            let kind = match count {
                2 => SelectionType::Semantic,
                3.. => SelectionType::Lines,
                _ => SelectionType::Simple,
            };
            let state = state.borrow();
            if let Some(engine) = state.engine.as_ref() {
                engine.term.lock().selection = Some(Selection::new(kind, point, Side::Left));
            }
            state.proxy.dirty.store(true, Ordering::Release);
        }
    });
    click.connect_released({
        let selection_drag_active = selection_drag_active.clone();
        let drag_autoscroll_generation = drag_autoscroll_generation.clone();
        let drag_autoscroll_active = drag_autoscroll_active.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        let state = state.clone();
        move |gesture, _, x, y| {
            selection_drag_active.set(false);
            if drag_autoscroll_active.replace(false) {
                drag_autoscroll_generation.set(drag_autoscroll_generation.get().wrapping_add(1));
                log::debug!("terminal selection edge autoscroll stopped");
            }
            drag_autoscroll_pointer.set(None);
            if let Some(point) = point_at(&state, x, y) {
                report_mouse_button(&state, point, 0, 1, false, gesture.current_event_state());
            }
        }
    });
    area.add_controller(click);

    let drag = gtk::GestureDrag::builder().button(1).build();
    let start = Rc::new(Cell::new((0.0, 0.0)));
    drag.connect_drag_begin({
        let selection_drag_active = selection_drag_active.clone();
        let start = start.clone();
        move |_, x, y| {
            selection_drag_active.set(true);
            start.set((x, y));
        }
    });
    drag.connect_drag_update({
        let area = area.clone();
        let drag_autoscroll_generation = drag_autoscroll_generation.clone();
        let drag_autoscroll_active = drag_autoscroll_active.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        let state = state.clone();
        let start = start.clone();
        move |gesture, dx, dy| {
            if state
                .borrow()
                .engine
                .as_ref()
                .is_some_and(TerminalEngine::mouse_mode)
                && !gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::SHIFT_MASK)
            {
                if drag_autoscroll_active.replace(false) {
                    drag_autoscroll_generation
                        .set(drag_autoscroll_generation.get().wrapping_add(1));
                }
                drag_autoscroll_pointer.set(None);
                return;
            }
            let (x, y) = start.get();
            let pointer_x = x + dx;
            let pointer_y = y + dy;
            let Some(point) = point_at(&state, pointer_x, pointer_y) else {
                return;
            };
            {
                let mut state = state.borrow_mut();
                if let Some(engine) = state.engine.as_ref()
                    && let Some(selection) = engine.term.lock().selection.as_mut()
                {
                    selection.update(point, Side::Right);
                }
                state.hovered_link = None;
                state.proxy.dirty.store(true, Ordering::Release);
            }
            area.set_cursor_from_name(Some("text"));
            schedule_selection_autoscroll(
                &area,
                &state,
                &drag_autoscroll_generation,
                &drag_autoscroll_active,
                &drag_autoscroll_pointer,
                pointer_x,
                pointer_y,
            );
        }
    });
    drag.connect_drag_end({
        let selection_drag_active = selection_drag_active.clone();
        let drag_autoscroll_generation = drag_autoscroll_generation.clone();
        let drag_autoscroll_active = drag_autoscroll_active.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |_, _, _| {
            selection_drag_active.set(false);
            if drag_autoscroll_active.replace(false) {
                drag_autoscroll_generation.set(drag_autoscroll_generation.get().wrapping_add(1));
                log::debug!("terminal selection edge autoscroll stopped");
            }
            drag_autoscroll_pointer.set(None);
        }
    });
    drag.connect_cancel({
        let selection_drag_active = selection_drag_active.clone();
        let drag_autoscroll_generation = drag_autoscroll_generation.clone();
        let drag_autoscroll_active = drag_autoscroll_active.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |_, _| {
            selection_drag_active.set(false);
            if drag_autoscroll_active.replace(false) {
                drag_autoscroll_generation.set(drag_autoscroll_generation.get().wrapping_add(1));
                log::debug!("terminal selection edge autoscroll cancelled");
            }
            drag_autoscroll_pointer.set(None);
        }
    });
    area.add_controller(drag);

    area.connect_unmap({
        let selection_drag_active = selection_drag_active.clone();
        let drag_autoscroll_generation = drag_autoscroll_generation.clone();
        let drag_autoscroll_active = drag_autoscroll_active.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |_| {
            selection_drag_active.set(false);
            if drag_autoscroll_active.replace(false) {
                drag_autoscroll_generation.set(drag_autoscroll_generation.get().wrapping_add(1));
                log::debug!("terminal selection edge autoscroll stopped on unmap");
            }
            drag_autoscroll_pointer.set(None);
        }
    });

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let area = area.clone();
        let selection_drag_active = selection_drag_active.clone();
        let state = state.clone();
        move |controller, x, y| {
            let Some(point) = point_at(&state, x, y) else {
                return;
            };
            let hovered_link = if selection_drag_active.get() {
                None
            } else {
                let state = state.borrow();
                state.engine.as_ref().and_then(|engine| {
                    let term = engine.term.lock();
                    link_at(&term, point).map(|(_, range)| range)
                })
            };
            {
                let mut state = state.borrow_mut();
                if state.hovered_link != hovered_link {
                    state.hovered_link = hovered_link;
                    state.proxy.dirty.store(true, Ordering::Release);
                }
            }
            area.set_cursor_from_name(Some(if hovered_link.is_some() {
                "pointer"
            } else {
                "text"
            }));

            let modifiers = controller.current_event_state();
            if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                return;
            }
            let mut state = state.borrow_mut();
            if state.last_mouse_point == Some(point) {
                return;
            }
            state.last_mouse_point = Some(point);
            let buttons = state.mouse_buttons;
            let Some(engine) = state.engine.as_ref() else {
                return;
            };
            let mode = *engine.term.lock().mode();
            let button = if buttons & 1 != 0 {
                Some(32)
            } else if buttons & 2 != 0 {
                Some(33)
            } else if buttons & 4 != 0 {
                Some(34)
            } else if mode.contains(TermMode::MOUSE_MOTION) {
                Some(35)
            } else {
                None
            };
            if let Some(button) = button
                && mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG)
            {
                engine.mouse_report(button, true, point, modifiers);
            }
        }
    });
    motion.connect_leave({
        let area = area.clone();
        let state = state.clone();
        move |_| {
            let mut state = state.borrow_mut();
            if state.hovered_link.take().is_some() {
                state.proxy.dirty.store(true, Ordering::Release);
            }
            area.set_cursor_from_name(None);
        }
    });
    area.add_controller(motion);

    let middle = gtk::GestureClick::builder().button(2).build();
    middle.connect_pressed({
        let state = state.clone();
        move |gesture, _, x, y| {
            if let Some(point) = point_at(&state, x, y)
                && report_mouse_button(&state, point, 1, 2, true, gesture.current_event_state())
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    });
    middle.connect_released({
        let state = state.clone();
        move |gesture, _, x, y| {
            if let Some(point) = point_at(&state, x, y) {
                report_mouse_button(&state, point, 1, 2, false, gesture.current_event_state());
            }
        }
    });
    area.add_controller(middle);
}

fn schedule_selection_autoscroll(
    area: &gtk::GLArea,
    state: &Rc<RefCell<UiState>>,
    drag_autoscroll_generation: &Rc<Cell<u64>>,
    drag_autoscroll_active: &Rc<Cell<bool>>,
    drag_autoscroll_pointer: &Rc<Cell<Option<(f64, f64)>>>,
    pointer_x: f64,
    pointer_y: f64,
) {
    let height = area.allocated_height().max(1) as f64;
    if (0.0..=height).contains(&pointer_y) {
        if drag_autoscroll_active.replace(false) {
            drag_autoscroll_generation.set(drag_autoscroll_generation.get().wrapping_add(1));
            log::debug!("terminal selection edge autoscroll stopped");
        }
        drag_autoscroll_pointer.set(None);
        return;
    }

    drag_autoscroll_pointer.set(Some((pointer_x, pointer_y)));
    if drag_autoscroll_active.get() {
        return;
    }

    let generation = drag_autoscroll_generation.get().wrapping_add(1);
    drag_autoscroll_generation.set(generation);
    drag_autoscroll_active.set(true);
    log::debug!(
        "terminal selection edge autoscroll started direction={}",
        if pointer_y < 0.0 { "up" } else { "down" }
    );

    let area = area.downgrade();
    let state = state.clone();
    let drag_autoscroll_generation = drag_autoscroll_generation.clone();
    let drag_autoscroll_active = drag_autoscroll_active.clone();
    let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if !drag_autoscroll_active.get() || drag_autoscroll_generation.get() != generation {
            return glib::ControlFlow::Break;
        }
        let Some(area) = area.upgrade() else {
            drag_autoscroll_active.set(false);
            drag_autoscroll_pointer.set(None);
            return glib::ControlFlow::Break;
        };
        let Some((pointer_x, pointer_y)) = drag_autoscroll_pointer.get() else {
            drag_autoscroll_active.set(false);
            return glib::ControlFlow::Break;
        };
        let height = area.allocated_height().max(1) as f64;
        if (0.0..=height).contains(&pointer_y) {
            drag_autoscroll_active.set(false);
            drag_autoscroll_pointer.set(None);
            return glib::ControlFlow::Break;
        }

        let state = state.borrow();
        let (Some(renderer), Some(engine)) = (state.renderer.as_ref(), state.engine.as_ref())
        else {
            drag_autoscroll_active.set(false);
            drag_autoscroll_pointer.set(None);
            return glib::ControlFlow::Break;
        };
        let size = renderer.size();
        let scale = renderer.scale() as f64;
        let logical_cell_height = (size.cell_height as f64 / scale).max(1.0);
        let overflow = if pointer_y < 0.0 {
            -pointer_y
        } else {
            pointer_y - height
        };
        let lines = (1 + (overflow / logical_cell_height).floor() as i32).min(8);
        let mut term = engine.term.lock();
        let previous_offset = term.grid().display_offset();
        term.scroll_display(Scroll::Delta(if pointer_y < 0.0 { lines } else { -lines }));
        let display_offset = term.grid().display_offset();
        if display_offset == previous_offset {
            drag_autoscroll_active.set(false);
            drag_autoscroll_pointer.set(None);
            return glib::ControlFlow::Break;
        }

        let column = ((pointer_x.max(0.0) * scale) / size.cell_width as f64).floor() as usize;
        let viewport_line = ((pointer_y.max(0.0) * scale) / size.cell_height as f64).floor() as i32;
        let point = Point::new(
            Line(
                viewport_line.min(term.screen_lines().saturating_sub(1) as i32)
                    - display_offset as i32,
            ),
            Column(column.min(term.columns().saturating_sub(1))),
        );
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, Side::Right);
        }
        state.proxy.dirty.store(true, Ordering::Release);
        glib::ControlFlow::Continue
    });
}

fn report_mouse_button(
    state: &Rc<RefCell<UiState>>,
    point: Point,
    button: u8,
    mask: u8,
    pressed: bool,
    modifiers: gdk::ModifierType,
) -> bool {
    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        return false;
    }
    let mut state = state.borrow_mut();
    let mouse_mode = state
        .engine
        .as_ref()
        .is_some_and(TerminalEngine::mouse_mode);
    if !mouse_mode {
        return false;
    }
    if pressed {
        state.mouse_buttons |= mask;
    } else {
        state.mouse_buttons &= !mask;
    }
    if let Some(engine) = state.engine.as_ref() {
        engine.mouse_report(button, pressed, point, modifiers);
    }
    true
}

fn point_at(state: &Rc<RefCell<UiState>>, x: f64, y: f64) -> Option<Point> {
    let state = state.borrow();
    let renderer = state.renderer.as_ref()?;
    let engine = state.engine.as_ref()?;
    let size = renderer.size();
    let scale = renderer.scale() as f64;
    let column = ((x.max(0.0) * scale) / size.cell_width as f64).floor() as usize;
    let viewport_line = ((y.max(0.0) * scale) / size.cell_height as f64).floor() as i32;
    let term = engine.term.lock();
    let column = Column(column.min(term.columns().saturating_sub(1)));
    let viewport_line = viewport_line.min(term.screen_lines().saturating_sub(1) as i32);
    let line = Line(viewport_line - term.grid().display_offset() as i32);
    Some(Point::new(line, column))
}

fn activate_at(state: &Rc<RefCell<UiState>>, point: Point) {
    let (activation, handlers) = {
        let state = state.borrow();
        let Some(engine) = state.engine.as_ref() else {
            return;
        };
        let term = engine.term.lock();
        let Some((target, _)) = link_at(&term, point) else {
            return;
        };
        let activation = if target.starts_with("http://") || target.starts_with("https://") {
            TerminalActivation::Url(target)
        } else {
            TerminalActivation::File(TerminalFileActivation {
                target,
                launch_dir: state.launch_dir.clone(),
            })
        };
        (activation, state.activation_handlers.clone())
    };
    for handler in handlers.borrow().iter() {
        handler(activation.clone());
    }
}

fn link_at(term: &Term<TerminalEventProxy>, point: Point) -> Option<(String, LinkRange)> {
    if let Some(hyperlink) = term.grid()[point].hyperlink() {
        let uri = hyperlink.uri().to_string();
        let mut start = point.column.0;
        while start > 0
            && term.grid()[Point::new(point.line, Column(start - 1))]
                .hyperlink()
                .is_some_and(|link| link.uri() == uri)
        {
            start -= 1;
        }
        let mut end = point.column.0;
        while end + 1 < term.columns()
            && term.grid()[Point::new(point.line, Column(end + 1))]
                .hyperlink()
                .is_some_and(|link| link.uri() == uri)
        {
            end += 1;
        }
        return Some((
            uri,
            LinkRange {
                start: Point::new(point.line, Column(start)),
                end: Point::new(point.line, Column(end)),
            },
        ));
    }

    let end = Point::new(point.line, Column(term.columns().saturating_sub(1)));
    let line = term.bounds_to_string(Point::new(point.line, Column(0)), end);
    let chars = line.char_indices().collect::<Vec<_>>();
    let is_delimiter =
        |character: char| character.is_whitespace() || matches!(character, '<' | '>' | '"' | '\'');
    let (byte, character) = chars.get(point.column.0).copied()?;
    if is_delimiter(character) {
        return None;
    }
    let start = line[..byte]
        .char_indices()
        .rev()
        .find(|(_, character)| is_delimiter(*character))
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or(0);
    let end = line[byte..]
        .char_indices()
        .find(|(_, character)| is_delimiter(*character))
        .map(|(index, _)| byte + index)
        .unwrap_or(line.len());
    let token = line[start..end]
        .trim_end_matches(|character: char| matches!(character, '.' | ',' | ';' | ')' | ']' | '}'));
    if token.is_empty()
        || !(token.contains('/')
            || token.contains('.')
            || token.starts_with("http://")
            || token.starts_with("https://"))
    {
        return None;
    }
    let start_column = line[..start].chars().count();
    let end_column = start_column + token.chars().count().saturating_sub(1);
    Some((
        token.to_string(),
        LinkRange {
            start: Point::new(point.line, Column(start_column)),
            end: Point::new(point.line, Column(end_column)),
        },
    ))
}

fn install_scroll(area: &gtk::GLArea, state: &Rc<RefCell<UiState>>) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll({
        let area = area.clone();
        let state = state.clone();
        move |controller, _, dy| {
            let mut state = state.borrow_mut();
            state.scroll_remainder += dy;
            let steps = state.scroll_remainder.trunc() as i32;
            state.scroll_remainder -= steps as f64;
            if steps != 0 {
                if let Some(engine) = state.engine.as_ref() {
                    let mode = *engine.term.lock().mode();
                    if mode.intersects(TermMode::MOUSE_MODE) {
                        let point = state.last_mouse_point.unwrap_or_default();
                        let button = if steps < 0 { 64 } else { 65 };
                        for _ in 0..steps.unsigned_abs() {
                            engine.mouse_report(
                                button,
                                true,
                                point,
                                controller.current_event_state(),
                            );
                        }
                    } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
                        && !controller
                            .current_event_state()
                            .contains(gdk::ModifierType::SHIFT_MASK)
                    {
                        let sequence = if steps < 0 { b"\x1bOA" } else { b"\x1bOB" };
                        for _ in 0..steps.unsigned_abs() {
                            engine.input(sequence.to_vec());
                        }
                    } else {
                        engine.scroll(-steps * SCROLL_LINES_PER_WHEEL_STEP);
                    }
                }
                state.hovered_link = None;
                area.set_cursor_from_name(Some("text"));
                state.proxy.dirty.store(true, Ordering::Release);
            }
            glib::Propagation::Stop
        }
    });
    area.add_controller(scroll);
}

fn install_middle_autoscroll(
    area: &gtk::GLArea,
    marker: &gtk::DrawingArea,
    state: &Rc<RefCell<UiState>>,
) {
    let autoscroll = Rc::new(canvas_scroll::MiddleAutoscroll::new());
    canvas_scroll::install_middle_autoscroll_marker(
        marker,
        &autoscroll,
        canvas_scroll::AutoscrollAxes::Vertical,
    );
    let line_remainder = Rc::new(Cell::new(0.0));
    canvas_scroll::install_middle_autoscroll(
        area,
        &autoscroll,
        canvas_scroll::AutoscrollAxes::Vertical,
        "terminal",
        {
            let state = state.clone();
            move || {
                state.borrow().engine.as_ref().is_some_and(|engine| {
                    let term = engine.term.lock();
                    term.grid().history_size() > 0
                })
            }
        },
        {
            let state = state.clone();
            let line_remainder = line_remainder.clone();
            move |autoscroll_state| {
                let logical_cell_height = {
                    let state = state.borrow();
                    let Some(renderer) = state.renderer.as_ref() else {
                        return;
                    };
                    renderer.size().cell_height as f64 / renderer.scale() as f64
                };
                let delta = canvas_scroll::middle_autoscroll_delta(
                    autoscroll_state.pointer.y - autoscroll_state.origin.y,
                ) / logical_cell_height.max(1.0);
                let accumulated = line_remainder.get() + delta;
                let lines = accumulated.trunc() as i32;
                line_remainder.set(accumulated - lines as f64);
                if lines == 0 {
                    return;
                }
                let mut state = state.borrow_mut();
                if let Some(engine) = state.engine.as_ref() {
                    engine.scroll(-lines);
                    state.hovered_link = None;
                    state.proxy.dirty.store(true, Ordering::Release);
                }
            }
        },
        {
            let line_remainder = line_remainder.clone();
            move || line_remainder.set(0.0)
        },
        {
            let line_remainder = line_remainder.clone();
            move || line_remainder.set(0.0)
        },
        {
            let area = area.clone();
            move |cursor| area.set_cursor_from_name(cursor.or(Some("text")))
        },
        {
            let marker = marker.clone();
            move || marker.queue_draw()
        },
    );
}

fn install_scrollbar(
    area: &gtk::GLArea,
    adjustment: &gtk::Adjustment,
    state: &Rc<RefCell<UiState>>,
) {
    let area = area.clone();
    let state = Rc::downgrade(state);
    adjustment.connect_value_changed(move |adjustment| {
        let Some(state) = state.upgrade() else {
            return;
        };
        let mut state = state.borrow_mut();
        if state.syncing_scrollbar {
            return;
        }
        let Some(engine) = state.engine.as_ref() else {
            return;
        };
        let mut term = engine.term.lock();
        let history_size = term.grid().history_size();
        let current_offset = term.grid().display_offset();
        let requested_offset = (history_size as f64 - adjustment.value())
            .round()
            .clamp(0.0, history_size as f64) as usize;
        if requested_offset != current_offset {
            term.scroll_display(Scroll::Delta(
                requested_offset as i32 - current_offset as i32,
            ));
            drop(term);
            state.hovered_link = None;
            area.set_cursor_from_name(None);
            state.proxy.dirty.store(true, Ordering::Release);
        }
    });
}

fn sync_scrollbar(state: &Rc<RefCell<UiState>>, scrollbar: &gtk::Scrollbar) {
    let (history_size, screen_lines, display_offset, adjustment) = {
        let state = state.borrow();
        let Some(engine) = state.engine.as_ref() else {
            return;
        };
        let term = engine.term.lock();
        (
            term.grid().history_size(),
            term.screen_lines(),
            term.grid().display_offset(),
            state.scrollbar_adjustment.clone(),
        )
    };
    let value = history_size.saturating_sub(display_offset) as f64;
    let upper = history_size.saturating_add(screen_lines) as f64;
    let page_size = screen_lines as f64;
    scrollbar.set_visible(history_size > 0);
    if (adjustment.value() - value).abs() < f64::EPSILON
        && (adjustment.upper() - upper).abs() < f64::EPSILON
        && (adjustment.page_size() - page_size).abs() < f64::EPSILON
    {
        return;
    }

    state.borrow_mut().syncing_scrollbar = true;
    adjustment.configure(
        value,
        0.0,
        upper,
        1.0,
        (page_size - 1.0).max(1.0),
        page_size,
    );
    state.borrow_mut().syncing_scrollbar = false;
}

fn install_focus(area: &gtk::GLArea, state: &Rc<RefCell<UiState>>) {
    let click = gtk::GestureClick::builder().button(0).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let area = area.clone();
        move |_, _, _, _| {
            area.grab_focus();
        }
    });
    area.add_controller(click);

    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter({
        let state = state.clone();
        move |_| set_focused(&state, true)
    });
    focus.connect_leave({
        let state = state.clone();
        move |_| set_focused(&state, false)
    });
    area.add_controller(focus);
}

fn set_focused(state: &Rc<RefCell<UiState>>, focused: bool) {
    let mut state = state.borrow_mut();
    if state.focused == focused {
        return;
    }
    state.focused = focused;
    if let Some(engine) = state.engine.as_ref() {
        let mut term = engine.term.lock();
        term.is_focused = focused;
        if term.mode().contains(TermMode::FOCUS_IN_OUT) {
            engine.input(if focused {
                &b"\x1b[I"[..]
            } else {
                &b"\x1b[O"[..]
            });
        }
    }
    state.proxy.dirty.store(true, Ordering::Release);
    log::debug!("alacritty terminal focus changed focused={focused}");
}

fn install_context_menu(area: &gtk::GLArea, state: &Rc<RefCell<UiState>>) {
    let click = gtk::GestureClick::builder().button(3).build();
    click.connect_pressed({
        let area = area.clone();
        let state = state.clone();
        move |gesture, _, x, y| {
            if let Some(point) = point_at(&state, x, y)
                && report_mouse_button(&state, point, 2, 4, true, gesture.current_event_state())
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }
            let (has_engine, has_selection) = {
                let state = state.borrow();
                (
                    state.engine.is_some(),
                    state
                        .engine
                        .as_ref()
                        .is_some_and(TerminalEngine::has_selection),
                )
            };
            let Some(parent) = area.parent() else {
                return;
            };
            let Some((popup_x, popup_y)) = area.translate_coordinates(&parent, x, y) else {
                log::debug!("terminal context menu skipped; GLArea coordinates did not translate");
                return;
            };
            let popover = context_menu::popup_action_menu(
                &parent,
                popup_x,
                popup_y,
                vec![
                    context_menu::ActionMenuSection::new(vec![
                        context_menu::ActionMenuItem::new(
                            "Copy",
                            TerminalContextAction::Copy,
                            has_selection,
                        ),
                        context_menu::ActionMenuItem::new(
                            "Copy Screen",
                            TerminalContextAction::CopyScreen,
                            has_engine,
                        ),
                        context_menu::ActionMenuItem::new(
                            "Copy All",
                            TerminalContextAction::CopyAll,
                            has_engine,
                        ),
                    ]),
                    context_menu::ActionMenuSection::new(vec![
                        context_menu::ActionMenuItem::new(
                            "Select All",
                            TerminalContextAction::SelectAll,
                            has_engine,
                        ),
                        context_menu::ActionMenuItem::new(
                            "Paste",
                            TerminalContextAction::Paste,
                            has_engine,
                        ),
                    ]),
                ],
                {
                    let area = area.clone();
                    let state = state.clone();

                    move |action| match action {
                        TerminalContextAction::Copy => {
                            if let Some(text) = state
                                .borrow()
                                .engine
                                .as_ref()
                                .and_then(TerminalEngine::selection_text)
                            {
                                area.clipboard().set_text(&text);
                            }
                        }
                        TerminalContextAction::CopyScreen => {
                            if let Some(text) = state
                                .borrow()
                                .engine
                                .as_ref()
                                .map(TerminalEngine::visible_text)
                            {
                                area.clipboard().set_text(&text);
                            }
                        }
                        TerminalContextAction::CopyAll => {
                            if let Some(text) =
                                state.borrow().engine.as_ref().map(TerminalEngine::all_text)
                            {
                                area.clipboard().set_text(&text);
                            }
                        }
                        TerminalContextAction::SelectAll => {
                            let state = state.borrow();
                            if let Some(engine) = state.engine.as_ref() {
                                engine.select_all();
                                state.proxy.dirty.store(true, Ordering::Release);
                            }
                        }
                        TerminalContextAction::Paste => paste_clipboard(&area, &state),
                    }
                },
            );
            popover.connect_closed(|popover| popover.unparent());
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    click.connect_released({
        let state = state.clone();
        move |gesture, _, x, y| {
            if let Some(point) = point_at(&state, x, y) {
                report_mouse_button(&state, point, 2, 4, false, gesture.current_event_state());
            }
        }
    });
    area.add_controller(click);
}

fn paste_clipboard(widget: &impl IsA<gtk::Widget>, state: &Rc<RefCell<UiState>>) {
    widget
        .clipboard()
        .read_text_async(None::<&gio::Cancellable>, {
            let state = state.clone();
            move |result| match result {
                Ok(Some(text)) => {
                    if let Some(engine) = state.borrow().engine.as_ref() {
                        engine.paste(text.to_string());
                    }
                }
                Ok(None) => {}
                Err(err) => log::warn!("alacritty terminal clipboard read failed: {err}"),
            }
        });
}

fn install_file_drop(area: &gtk::GLArea, state: &Rc<RefCell<UiState>>) {
    let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    target.connect_drop({
        let area = area.clone();
        let state = state.clone();
        move |_, value, _, _| {
            let Ok(files) = value.get::<gdk::FileList>() else {
                return false;
            };
            let paths = files
                .files()
                .into_iter()
                .filter_map(|file| file.path())
                .collect::<Vec<_>>();
            if paths.is_empty() {
                return false;
            }
            let text = paths
                .iter()
                .map(|path| {
                    let value = path.to_string_lossy();
                    format!("'{}'", value.replace('\'', "'\"'\"'"))
                })
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(engine) = state.borrow().engine.as_ref() {
                engine.paste(text);
            }
            area.grab_focus();
            log::info!(
                "alacritty terminal file drop pasted paths count={}",
                paths.len()
            );
            true
        }
    });
    area.add_controller(target);
}

fn drain_events(area: &gtk::GLArea, state: &Rc<RefCell<UiState>>) {
    loop {
        let event = match state.borrow().receiver.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        };
        match event {
            Event::Title(title) => {
                let handlers = state.borrow().title_handlers.clone();
                for handler in handlers.borrow().iter() {
                    handler(title.clone());
                }
            }
            Event::ResetTitle => {
                let handlers = state.borrow().title_handlers.clone();
                for handler in handlers.borrow().iter() {
                    handler(String::new());
                }
            }
            Event::ClipboardStore(_, text) if state.borrow().focused => {
                area.clipboard().set_text(&text)
            }
            Event::ClipboardStore(_, _) => {}
            Event::ClipboardLoad(_, formatter) => {
                if !state.borrow().focused {
                    continue;
                }
                area.clipboard()
                    .read_text_async(None::<&gio::Cancellable>, {
                        let state = state.clone();
                        move |result| {
                            if let Ok(Some(text)) = result {
                                let text = formatter(&text);
                                if let Some(engine) = state.borrow().engine.as_ref() {
                                    engine.input(text.into_bytes());
                                }
                            }
                        }
                    });
            }
            Event::ColorRequest(index, formatter) => {
                let state = state.borrow();
                if let Some(engine) = state.engine.as_ref() {
                    let color = renderer::color_for_index(engine.term.lock().colors(), index);
                    engine.input(formatter(color).into_bytes());
                }
            }
            Event::PtyWrite(text) => {
                if let Some(engine) = state.borrow().engine.as_ref() {
                    engine.input(text.into_bytes());
                }
            }
            Event::TextAreaSizeRequest(formatter) => {
                let state = state.borrow();
                if let (Some(engine), Some(size)) = (
                    state.engine.as_ref(),
                    state
                        .renderer
                        .as_ref()
                        .map(GlRenderer::size)
                        .map(WindowSize::from),
                ) {
                    engine.input(formatter(size).into_bytes());
                }
            }
            Event::ChildExit(code) => {
                let handlers = {
                    let mut state = state.borrow_mut();
                    if state.exited {
                        continue;
                    }
                    state.exited = true;
                    state.exit_handlers.clone()
                };
                log::info!("alacritty terminal child exited status={code:?}");
                for handler in handlers.borrow().iter() {
                    handler(code.clone());
                }
            }
            Event::Exit => log::debug!("alacritty terminal PTY event loop exited"),
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::Bell => {
            }
        }
    }
}
