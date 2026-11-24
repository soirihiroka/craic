use craic_language::LinkTarget;
use craic_platform::terminal::{TerminalBackend, TerminalCommand, TerminalLifecycle, TerminalSize};
use std::borrow::Cow;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::watch;

use skia_safe::{AlphaType, ColorType, Data, Image, ImageInfo, images};

mod ghostty;
mod pty;

use ghostty::Emulator;
use pty::PtyWorker;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalViewport {
    pub columns: usize,
    pub lines: usize,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl TerminalViewport {
    pub fn new(columns: usize, lines: usize, cell_width: u16, cell_height: u16) -> Self {
        Self {
            columns: columns.clamp(2, u16::MAX as usize),
            lines: lines.clamp(1, u16::MAX as usize),
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalColor(pub u8, pub u8, pub u8);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCellStyle {
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikeout: bool,
    pub underline: bool,
    pub wide: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub(crate) explicit_background: bool,
    pub style: TerminalCellStyle,
    pub selected: bool,
    pub hyperlink: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalCursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalScroll {
    Lines(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
    Row(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSelectionType {
    Simple,
    Block,
    Semantic,
    Lines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSearchDirection {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSearchMatch {
    pub start_line: i32,
    pub start_column: usize,
    pub end_line: i32,
    pub end_column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMouseButton {
    None,
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMouseAction {
    Press,
    Release,
    Move,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalMouseModifiers {
    pub shift: bool,
    pub option: bool,
    pub control: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursor {
    pub line: usize,
    pub column: usize,
    pub shape: TerminalCursorShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageLayer {
    BelowBackground,
    BelowText,
    AboveText,
}

#[derive(Clone)]
pub struct TerminalImage {
    pub id: u64,
    pub line: i32,
    pub column: i32,
    pub width: usize,
    pub height: usize,
    pub(crate) generation: u64,
    pub(crate) offset: (u32, u32),
    pub(crate) source: (u32, u32, u32, u32),
    pub(crate) layer: ImageLayer,
    pub(crate) image: Image,
}

impl std::fmt::Debug for TerminalImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalImage")
            .field("id", &self.id)
            .field("line", &self.line)
            .field("column", &self.column)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl PartialEq for TerminalImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.line == other.line
            && self.column == other.column
            && self.width == other.width
            && self.height == other.height
            && self.generation == other.generation
            && self.offset == other.offset
            && self.source == other.source
            && self.layer == other.layer
    }
}

impl Eq for TerminalImage {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub columns: usize,
    pub lines: usize,
    pub display_offset: usize,
    pub cursor: Option<TerminalCursor>,
    pub background: TerminalColor,
    pub cells: Vec<TerminalCell>,
    pub images: Vec<TerminalImage>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalClipboard {
    #[default]
    Clipboard,
    Selection,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalEventBatch {
    pub needs_redraw: bool,
    pub title: Option<Option<String>>,
    pub activity: Option<bool>,
    pub next_animation: Option<Duration>,
    pub clipboard_store: Option<(TerminalClipboard, String)>,
    pub bell: bool,
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub exit_status: Option<std::process::ExitStatus>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalKey {
    Character(char),
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Function(u8),
    Keypad(u8),
    KeypadEnter,
    KeypadAdd,
    KeypadSubtract,
    KeypadMultiply,
    KeypadDivide,
    KeypadDecimal,
    KeypadEqual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalKeyAction {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalKeyModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub super_key: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
}

pub struct TerminalKeyEvent<'a> {
    pub key: TerminalKey,
    pub action: TerminalKeyAction,
    pub modifiers: TerminalKeyModifiers,
    pub consumed_modifiers: TerminalKeyModifiers,
    pub text: &'a str,
    pub unshifted: Option<char>,
}

struct TerminalState {
    emulator: Emulator,
    viewport: TerminalViewport,
    wake: watch::Sender<()>,
}

impl TerminalState {
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.emulator.feed(bytes);
        self.wake.send_replace(());
        self.emulator.take_reply()
    }
}

pub struct TerminalSession {
    state: Arc<Mutex<TerminalState>>,
    pty: Option<PtyWorker>,
}

impl TerminalSession {
    pub fn new(viewport: TerminalViewport) -> io::Result<Self> {
        let viewport = TerminalViewport::new(
            viewport.columns,
            viewport.lines,
            viewport.cell_width,
            viewport.cell_height,
        );
        Ok(Self {
            state: Arc::new(Mutex::new(TerminalState {
                emulator: Emulator::new(viewport)?,
                viewport,
                wake: watch::Sender::new(()),
            })),
            pty: None,
        })
    }

    pub fn spawn(command: TerminalCommand, viewport: TerminalViewport) -> io::Result<Self> {
        let mut terminal = Self::new(viewport)?;
        let viewport = terminal.lock().viewport;
        terminal.pty = Some(PtyWorker::spawn(
            command,
            viewport,
            Arc::clone(&terminal.state),
        )?);
        Ok(terminal)
    }

    fn lock(&self) -> MutexGuard<'_, TerminalState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn child_pid(&self) -> u32 {
        self.pty.as_ref().map_or(0, PtyWorker::pid)
    }

    pub fn foreground_process_group(&self) -> Option<i32> {
        self.pty.as_ref().filter(|pty| pty.running())?.foreground()
    }

    pub fn feed(&self, bytes: &[u8]) {
        let reply = self.lock().feed(bytes);
        if let Some(pty) = &self.pty {
            let _ = pty.input(reply);
        }
    }

    pub fn input(&self, bytes: impl Into<Cow<'static, [u8]>>) -> io::Result<()> {
        self.pty
            .as_ref()
            .filter(|pty| pty.running())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "Terminal process is not running")
            })?
            .input(bytes.into().into_owned())
    }

    pub fn send_key(&self, event: TerminalKeyEvent<'_>) -> io::Result<bool> {
        let bytes = self.lock().emulator.key(event)?;
        if bytes.is_empty() {
            return Ok(false);
        }
        self.input(bytes)?;
        self.scroll(TerminalScroll::Bottom);
        Ok(true)
    }

    pub fn paste(&self, text: &str) -> io::Result<()> {
        let bytes = self.lock().emulator.paste(text)?;
        self.input(bytes)?;
        self.scroll(TerminalScroll::Bottom);
        Ok(())
    }

    pub fn resize(&mut self, viewport: TerminalViewport) -> io::Result<()> {
        let viewport = TerminalViewport::new(
            viewport.columns,
            viewport.lines,
            viewport.cell_width,
            viewport.cell_height,
        );
        let mut state = self.lock();
        if state.viewport != viewport {
            state.emulator.resize(viewport)?;
            state.viewport = viewport;
        }
        // A resize can generate a mode-2048 in-band pixel-size report.
        if let Some(pty) = self.pty.as_ref().filter(|pty| pty.running()) {
            pty.resize(viewport)?;
            pty.input(state.emulator.take_reply())?;
        }
        Ok(())
    }

    pub fn scrollbar(&self) -> (usize, usize, usize) {
        self.lock().emulator.scrollbar()
    }

    pub fn scroll(&self, scroll: TerminalScroll) {
        self.lock().emulator.scroll(scroll);
    }

    pub fn begin_selection(
        &self,
        line: usize,
        column: usize,
        kind: TerminalSelectionType,
        side: TerminalSide,
    ) {
        self.lock()
            .emulator
            .begin_selection(line, column, kind, side);
    }

    pub fn update_selection(&self, line: usize, column: usize, side: TerminalSide) {
        self.lock().emulator.update_selection(line, column, side);
    }

    pub fn clear_selection(&self) {
        self.lock().emulator.clear_selection();
    }

    pub fn select_all(&self) {
        self.lock().emulator.select_all();
    }

    pub fn selected_text(&self) -> Option<String> {
        self.lock().emulator.selected_text()
    }

    pub fn visible_text(&self) -> String {
        let state = self.lock();
        let (offset, len, _) = state.emulator.scrollbar();
        state.emulator.text_rows(offset, offset + len)
    }

    pub fn all_text(&self) -> String {
        self.lock().emulator.all_text()
    }

    pub fn text_before_cursor(&self, max_lines: usize) -> String {
        let state = self.lock();
        let end = state.emulator.history_size() + state.emulator.cursor_position().0;
        state.emulator.text_rows(end.saturating_sub(max_lines), end)
    }

    pub fn recent_text(&self, max_lines: usize) -> String {
        let state = self.lock();
        let (_, _, end) = state.emulator.scrollbar();
        state.emulator.text_rows(end.saturating_sub(max_lines), end)
    }

    pub fn cursor_row(&self) -> usize {
        let state = self.lock();
        state.emulator.history_size() + state.emulator.cursor_position().0
    }

    pub fn search(
        &self,
        pattern: &str,
        direction: TerminalSearchDirection,
    ) -> Result<Option<TerminalSearchMatch>, String> {
        self.lock().emulator.search(pattern, direction)
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.lock().emulator.mouse_reporting_enabled()
    }

    pub fn report_mouse_at(
        &self,
        button: TerminalMouseButton,
        action: TerminalMouseAction,
        modifiers: TerminalMouseModifiers,
        x: f32,
        y: f32,
    ) -> bool {
        let bytes = self.lock().emulator.mouse(button, action, modifiers, x, y);
        bytes.is_ok_and(|bytes| !bytes.is_empty() && self.input(bytes).is_ok())
    }

    pub fn report_focus(&self, focused: bool) {
        let bytes = self.lock().emulator.focus(focused);
        let _ = self.input(bytes);
    }

    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.lock().wake.subscribe()
    }

    pub fn drain_events(&self) -> TerminalEventBatch {
        let mut state = self.lock();
        let mut batch = state.emulator.drain_events();
        if let Some(status) = self.pty.as_ref().and_then(PtyWorker::take_exit) {
            batch.exited = true;
            batch.exit_status = status;
            batch.exit_code = status.and_then(|status| status.code());
            batch.needs_redraw = true;
        }
        batch
    }

    pub fn link_at(&self, line: usize, column: usize) -> Option<LinkTarget> {
        self.lock().emulator.link_at(line, column)
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let mut state = self.lock();
        state.emulator.snapshot().unwrap_or_else(|error| {
            log::warn!("terminal render state unavailable: {error}");
            TerminalSnapshot {
                columns: state.viewport.columns,
                lines: state.viewport.lines,
                display_offset: 0,
                cursor: None,
                background: TerminalColor(30, 30, 30),
                cells: Vec::new(),
                images: Vec::new(),
            }
        })
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        self.pty.as_mut().map_or(Ok(()), PtyWorker::stop)
    }

    pub fn begin_shutdown(&mut self) {
        if let Some(pty) = &mut self.pty {
            pty.begin_shutdown();
        }
    }
}

impl TerminalBackend for TerminalSession {
    fn process_id(&self) -> Option<u32> {
        self.pty
            .as_ref()
            .filter(|pty| pty.running())
            .map(PtyWorker::pid)
    }

    fn lifecycle(&self) -> TerminalLifecycle {
        match &self.pty {
            Some(pty) if pty.closing() => TerminalLifecycle::Closing,
            Some(pty) if pty.running() => TerminalLifecycle::Running,
            _ => TerminalLifecycle::Exited,
        }
    }

    fn send_input(&self, bytes: &[u8]) -> Result<(), String> {
        self.input(bytes.to_vec())
            .map_err(|error| error.to_string())
    }

    fn resize_terminal(&mut self, size: TerminalSize) -> Result<(), String> {
        self.resize(TerminalViewport::new(
            size.columns,
            size.rows,
            size.cell_width,
            size.cell_height,
        ))
        .map_err(|error| error.to_string())
    }

    fn terminate_process(&mut self) -> Result<(), String> {
        self.pty
            .as_mut()
            .map_or(Ok(()), PtyWorker::stop)
            .map_err(|error| error.to_string())
    }
}
