use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side as GridSide};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config, Term, TermMode, point_to_viewport, viewport_to_point};
use alacritty_terminal::tty::{self, EventedPty, EventedReadWrite, Options, Pty, Shell};
use alacritty_terminal::vte::ansi::{Color, CursorShape};
#[cfg(unix)]
use polling::{Event as PollingEvent, PollMode, Poller};
use skia_safe::{AlphaType, ColorType, Data, Image, ImageInfo, images};

use crate::sixel::{SixelDecoded, SixelStreamParser};

const TERMINAL_EVENT_CAPACITY: usize = 512;
const SIXEL_EVENT_CAPACITY: usize = 8;
const MAX_SIXEL_IMAGES: usize = 32;
const MAX_SIXEL_IMAGE_BYTES: usize = 16 * 1024 * 1024;
static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalSpawnOptions {
    pub working_directory: Option<PathBuf>,
    pub shell_program: Option<String>,
    pub shell_arguments: Vec<String>,
    pub environment: HashMap<String, String>,
}

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
            columns: columns.max(alacritty_terminal::term::MIN_COLUMNS),
            lines: lines.max(alacritty_terminal::term::MIN_SCREEN_LINES),
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
        }
    }

    fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: u16::try_from(self.lines).unwrap_or(u16::MAX),
            num_cols: u16::try_from(self.columns).unwrap_or(u16::MAX),
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Dimensions for TerminalViewport {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalColor {
    Named(u16),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

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

#[derive(Clone)]
pub struct TerminalImage {
    pub id: u64,
    pub line: i32,
    pub column: usize,
    pub width: usize,
    pub height: usize,
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
    }
}

impl Eq for TerminalImage {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub columns: usize,
    pub lines: usize,
    pub display_offset: usize,
    pub cursor: Option<TerminalCursor>,
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
    pub clipboard_store: Option<(TerminalClipboard, String)>,
    pub bell: bool,
    pub exited: bool,
    pub exit_code: Option<i32>,
}

#[derive(Clone)]
struct TerminalEventProxy {
    sender: SyncSender<Event>,
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        match self.sender.try_send(event) {
            Ok(()) | Err(TrySendError::Full(Event::Wakeup | Event::MouseCursorDirty)) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

#[cfg(unix)]
struct SixelTeeReader {
    file: std::fs::File,
    parser: SixelStreamParser,
    images: SyncSender<SixelDecoded>,
    event_proxy: TerminalEventProxy,
}

#[cfg(unix)]
impl Read for SixelTeeReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.file.read(buffer)?;
        if count == 0 {
            return Ok(0);
        }
        for image in self.parser.feed(&buffer[..count]) {
            if self.images.try_send(image).is_ok() {
                self.event_proxy.send_event(Event::Wakeup);
            }
        }
        Ok(count)
    }
}

#[cfg(unix)]
struct SixelPty {
    inner: Pty,
    reader: SixelTeeReader,
}

#[cfg(unix)]
impl SixelPty {
    fn new(
        inner: Pty,
        images: SyncSender<SixelDecoded>,
        output_line: Arc<AtomicU64>,
        event_proxy: TerminalEventProxy,
    ) -> io::Result<Self> {
        let file = inner.file().try_clone()?;
        Ok(Self {
            inner,
            reader: SixelTeeReader {
                file,
                parser: SixelStreamParser::new(output_line),
                images,
                event_proxy,
            },
        })
    }
}

#[cfg(unix)]
impl EventedReadWrite for SixelPty {
    type Reader = SixelTeeReader;
    type Writer = std::fs::File;

    unsafe fn register(
        &mut self,
        poller: &Arc<Poller>,
        interest: PollingEvent,
        mode: PollMode,
    ) -> io::Result<()> {
        unsafe { self.inner.register(poller, interest, mode) }
    }

    fn reregister(
        &mut self,
        poller: &Arc<Poller>,
        interest: PollingEvent,
        mode: PollMode,
    ) -> io::Result<()> {
        self.inner.reregister(poller, interest, mode)
    }

    fn deregister(&mut self, poller: &Arc<Poller>) -> io::Result<()> {
        self.inner.deregister(poller)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.inner.writer()
    }
}

#[cfg(unix)]
impl EventedPty for SixelPty {
    fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
        self.inner.next_child_event()
    }
}

#[cfg(unix)]
impl OnResize for SixelPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.inner.on_resize(window_size);
    }
}

#[cfg(unix)]
type TerminalPty = SixelPty;
#[cfg(not(unix))]
type TerminalPty = Pty;

type TerminalIoThread = JoinHandle<(EventLoop<TerminalPty, TerminalEventProxy>, State)>;

struct StoredSixelImage {
    id: u64,
    output_line: u64,
    column: usize,
    width: usize,
    height: usize,
    bytes: usize,
    image: Image,
}

#[derive(Default)]
struct StoredSixelState {
    images: VecDeque<StoredSixelImage>,
    total_bytes: usize,
    next_id: u64,
}

struct SixelPlane {
    receiver: Receiver<SixelDecoded>,
    output_line: Arc<AtomicU64>,
    state: Mutex<StoredSixelState>,
}

impl SixelPlane {
    fn new(receiver: Receiver<SixelDecoded>, output_line: Arc<AtomicU64>) -> Self {
        Self {
            receiver,
            output_line,
            state: Mutex::new(StoredSixelState::default()),
        }
    }

    fn snapshot(
        &self,
        cursor_line: i32,
        cursor_column: usize,
        display_offset: usize,
        viewport_lines: usize,
        cell_height: u16,
    ) -> Vec<TerminalImage> {
        let current_output_line = self.output_line.load(Ordering::Relaxed);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while let Ok(decoded) = self.receiver.try_recv() {
            let bytes = decoded.rgba.len();
            if bytes > MAX_SIXEL_IMAGE_BYTES {
                log::warn!(
                    "discarding oversized terminal Sixel image width={} height={} bytes={bytes}",
                    decoded.width,
                    decoded.height
                );
                continue;
            }
            let info = ImageInfo::new(
                (decoded.width as i32, decoded.height as i32),
                ColorType::RGBA8888,
                AlphaType::Unpremul,
                None,
            );
            let Some(image) = images::raster_from_data(
                &info,
                Data::new_copy(&decoded.rgba),
                decoded.width.saturating_mul(4),
            ) else {
                log::warn!(
                    "discarding terminal Sixel image that Skia could not decode width={} height={}",
                    decoded.width,
                    decoded.height
                );
                continue;
            };
            while state.images.len() >= MAX_SIXEL_IMAGES
                || state.total_bytes.saturating_add(bytes) > MAX_SIXEL_IMAGE_BYTES
            {
                let Some(removed) = state.images.pop_front() else {
                    break;
                };
                state.total_bytes = state.total_bytes.saturating_sub(removed.bytes);
            }
            state.next_id = state.next_id.wrapping_add(1).max(1);
            let id = state.next_id;
            state.total_bytes = state.total_bytes.saturating_add(bytes);
            state.images.push_back(StoredSixelImage {
                id,
                output_line: decoded.output_line,
                column: if current_output_line == decoded.output_line {
                    cursor_column
                } else {
                    0
                },
                width: decoded.width,
                height: decoded.height,
                bytes,
                image,
            });
            log::info!(
                "decoded terminal Sixel image id={id} width={} height={} retained_bytes={}",
                decoded.width,
                decoded.height,
                state.total_bytes
            );
        }

        let cell_height = usize::from(cell_height.max(1));
        state
            .images
            .iter()
            .filter_map(|image| {
                let output_delta = current_output_line.saturating_sub(image.output_line);
                let line = i64::from(cursor_line) - i64::try_from(output_delta).unwrap_or(i64::MAX)
                    + i64::try_from(display_offset).unwrap_or(i64::MAX);
                let occupied_lines = image.height.div_ceil(cell_height) as i64;
                (line < viewport_lines as i64 && line.saturating_add(occupied_lines) > 0).then(
                    || TerminalImage {
                        id: image.id,
                        line: i32::try_from(line).unwrap_or(if line < 0 {
                            i32::MIN
                        } else {
                            i32::MAX
                        }),
                        column: image.column,
                        width: image.width,
                        height: image.height,
                        image: image.image.clone(),
                    },
                )
            })
            .collect()
    }
}

pub struct TerminalSession {
    terminal: Arc<FairMutex<Term<TerminalEventProxy>>>,
    sender: alacritty_terminal::event_loop::EventLoopSender,
    events: Receiver<Event>,
    io_thread: Option<TerminalIoThread>,
    viewport: TerminalViewport,
    child_pid: u32,
    child_exited: AtomicBool,
    sixel: SixelPlane,
}

impl TerminalSession {
    pub fn spawn(options: TerminalSpawnOptions, viewport: TerminalViewport) -> io::Result<Self> {
        let (event_sender, events) = sync_channel(TERMINAL_EVENT_CAPACITY);
        let (sixel_sender, sixel_receiver) = sync_channel(SIXEL_EVENT_CAPACITY);
        let sixel_output_line = Arc::new(AtomicU64::new(0));
        let event_proxy = TerminalEventProxy {
            sender: event_sender,
        };
        let mut environment = options.environment;
        environment
            .entry("TERM".to_owned())
            .or_insert_with(|| "xterm-256color".to_owned());
        environment
            .entry("COLORTERM".to_owned())
            .or_insert_with(|| "truecolor".to_owned());
        environment
            .entry("TERM_PROGRAM".to_owned())
            .or_insert_with(|| "Craic".to_owned());
        let pty_options = Options {
            shell: options
                .shell_program
                .map(|program| Shell::new(program, options.shell_arguments)),
            working_directory: options.working_directory,
            drain_on_exit: false,
            env: environment,
        };
        let window_id = NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed);
        let pty = tty::new(&pty_options, viewport.window_size(), window_id)?;
        let child_pid = pty.child().id();
        #[cfg(unix)]
        let pty = SixelPty::new(
            pty,
            sixel_sender,
            Arc::clone(&sixel_output_line),
            event_proxy.clone(),
        )?;
        #[cfg(not(unix))]
        drop(sixel_sender);
        let terminal = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &viewport,
            event_proxy.clone(),
        )));
        terminal.lock().is_focused = true;
        let event_loop = EventLoop::new(
            Arc::clone(&terminal),
            event_proxy,
            pty,
            pty_options.drain_on_exit,
            false,
        )?;
        let sender = event_loop.channel();
        let io_thread = Some(event_loop.spawn());
        Ok(Self {
            terminal,
            sender,
            events,
            io_thread,
            viewport,
            child_pid,
            child_exited: AtomicBool::new(false),
            sixel: SixelPlane::new(sixel_receiver, sixel_output_line),
        })
    }

    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    pub fn input(&self, bytes: impl Into<Cow<'static, [u8]>>) -> io::Result<()> {
        self.sender
            .send(Msg::Input(bytes.into()))
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub fn resize(&mut self, viewport: TerminalViewport) -> io::Result<()> {
        if self.viewport == viewport {
            return Ok(());
        }
        self.terminal.lock().resize(viewport);
        self.sender
            .send(Msg::Resize(viewport.window_size()))
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.viewport = viewport;
        Ok(())
    }

    pub fn scroll(&self, scroll: TerminalScroll) {
        self.terminal.lock().scroll_display(match scroll {
            TerminalScroll::Lines(lines) => Scroll::Delta(lines),
            TerminalScroll::PageUp => Scroll::PageUp,
            TerminalScroll::PageDown => Scroll::PageDown,
            TerminalScroll::Top => Scroll::Top,
            TerminalScroll::Bottom => Scroll::Bottom,
        });
    }

    pub fn begin_selection(
        &self,
        line: usize,
        column: usize,
        selection_type: TerminalSelectionType,
        side: TerminalSide,
    ) {
        let mut terminal = self.terminal.lock();
        let point = viewport_to_point(
            terminal.grid().display_offset(),
            Point::new(
                line.min(terminal.screen_lines().saturating_sub(1)),
                Column(column.min(terminal.columns().saturating_sub(1))),
            ),
        );
        terminal.selection = Some(Selection::new(
            match selection_type {
                TerminalSelectionType::Simple => SelectionType::Simple,
                TerminalSelectionType::Block => SelectionType::Block,
                TerminalSelectionType::Semantic => SelectionType::Semantic,
                TerminalSelectionType::Lines => SelectionType::Lines,
            },
            point,
            match side {
                TerminalSide::Left => Direction::Left,
                TerminalSide::Right => Direction::Right,
            },
        ));
    }

    pub fn update_selection(&self, line: usize, column: usize, side: TerminalSide) {
        let mut terminal = self.terminal.lock();
        let point = viewport_to_point(
            terminal.grid().display_offset(),
            Point::new(
                line.min(terminal.screen_lines().saturating_sub(1)),
                Column(column.min(terminal.columns().saturating_sub(1))),
            ),
        );
        if let Some(selection) = terminal.selection.as_mut() {
            selection.update(
                point,
                match side {
                    TerminalSide::Left => Direction::Left,
                    TerminalSide::Right => Direction::Right,
                },
            );
        }
    }

    pub fn clear_selection(&self) {
        self.terminal.lock().selection = None;
    }

    pub fn selected_text(&self) -> Option<String> {
        self.terminal.lock().selection_to_string()
    }

    pub fn visible_text(&self) -> String {
        let terminal = self.terminal.lock();
        let display_offset = terminal.grid().display_offset();
        let start = viewport_to_point(display_offset, Point::new(0, Column(0)));
        let end = viewport_to_point(
            display_offset,
            Point::new(
                terminal.screen_lines().saturating_sub(1),
                terminal.last_column(),
            ),
        );
        terminal.bounds_to_string(start, end)
    }

    pub fn all_text(&self) -> String {
        let terminal = self.terminal.lock();
        terminal.bounds_to_string(
            Point::new(terminal.topmost_line(), Column(0)),
            Point::new(terminal.bottommost_line(), terminal.last_column()),
        )
    }

    pub fn search(
        &self,
        pattern: &str,
        direction: TerminalSearchDirection,
    ) -> Result<Option<TerminalSearchMatch>, String> {
        let mut terminal = self.terminal.lock();
        if pattern.is_empty() {
            terminal.selection = None;
            return Ok(None);
        }
        let mut regex = RegexSearch::new(pattern).map_err(|error| error.to_string())?;
        let origin = terminal_search_origin(&terminal, direction);
        let (grid_direction, side) = match direction {
            TerminalSearchDirection::Previous => (Direction::Left, GridSide::Left),
            TerminalSearchDirection::Next => (Direction::Right, GridSide::Right),
        };
        let Some(found) = terminal.search_next(&mut regex, origin, grid_direction, side, None)
        else {
            return Ok(None);
        };
        let start = *found.start();
        let end = *found.end();
        let mut selection = Selection::new(SelectionType::Simple, start, Direction::Left);
        selection.update(end, Direction::Right);
        selection.include_all();
        terminal.selection = Some(selection);

        let target_offset = usize::try_from(-start.line.0).unwrap_or_default();
        let current_offset = terminal.grid().display_offset();
        let delta = target_offset as i32 - current_offset as i32;
        if delta != 0 {
            terminal.scroll_display(Scroll::Delta(delta));
        }

        Ok(Some(TerminalSearchMatch {
            start_line: start.line.0,
            start_column: start.column.0,
            end_line: end.line.0,
            end_column: end.column.0,
        }))
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.terminal.lock().mode().intersects(TermMode::MOUSE_MODE)
    }

    pub fn report_mouse(
        &self,
        button: TerminalMouseButton,
        action: TerminalMouseAction,
        modifiers: TerminalMouseModifiers,
        line: usize,
        column: usize,
    ) -> bool {
        let mode = *self.terminal.lock().mode();
        if !mode.intersects(TermMode::MOUSE_MODE)
            || (action == TerminalMouseAction::Move
                && ((button == TerminalMouseButton::None
                    && !mode.contains(TermMode::MOUSE_MOTION))
                    || (button != TerminalMouseButton::None
                        && !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG))))
        {
            return false;
        }
        let mut code = match button {
            TerminalMouseButton::None => 3,
            TerminalMouseButton::Left => 0,
            TerminalMouseButton::Middle => 1,
            TerminalMouseButton::Right => 2,
            TerminalMouseButton::WheelUp => 64,
            TerminalMouseButton::WheelDown => 65,
        };
        if action == TerminalMouseAction::Release && !mode.contains(TermMode::SGR_MOUSE) {
            code = 3;
        }
        if action == TerminalMouseAction::Move {
            code += 32;
        }
        if modifiers.shift {
            code += 4;
        }
        if modifiers.option {
            code += 8;
        }
        if modifiers.control {
            code += 16;
        }
        let column = column.saturating_add(1);
        let line = line.saturating_add(1);
        let sequence = if mode.contains(TermMode::SGR_MOUSE) {
            format!(
                "\x1b[<{code};{column};{line}{}",
                if action == TerminalMouseAction::Release {
                    'm'
                } else {
                    'M'
                }
            )
            .into_bytes()
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            let mut sequence = b"\x1b[M".to_vec();
            for value in [
                code + 32,
                column.saturating_add(32),
                line.saturating_add(32),
            ] {
                let character = char::from_u32(u32::try_from(value.min(2_047)).unwrap_or(2_047))
                    .unwrap_or('\u{fffd}');
                let mut bytes = [0; 4];
                sequence.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
            }
            sequence
        } else {
            vec![
                0x1b,
                b'[',
                b'M',
                u8::try_from((code + 32).min(255)).unwrap_or(255),
                u8::try_from(column.saturating_add(32).min(255)).unwrap_or(255),
                u8::try_from(line.saturating_add(32).min(255)).unwrap_or(255),
            ]
        };
        self.input(sequence).is_ok()
    }

    pub fn report_focus(&self, focused: bool) {
        if self.terminal.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
            let _ = self.input(if focused { b"\x1b[I" } else { b"\x1b[O" }.to_vec());
        }
    }

    pub fn try_recv_event(&self) -> Option<Event> {
        match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    pub fn drain_events(&self, clipboard: Option<&str>) -> TerminalEventBatch {
        self.drain_events_with_clipboard(|| clipboard.map(ToOwned::to_owned))
    }

    pub fn drain_events_with_clipboard(
        &self,
        mut clipboard: impl FnMut() -> Option<String>,
    ) -> TerminalEventBatch {
        let mut batch = TerminalEventBatch::default();
        while let Some(event) = self.try_recv_event() {
            match event {
                Event::MouseCursorDirty | Event::Wakeup | Event::CursorBlinkingChange => {
                    batch.needs_redraw = true;
                }
                Event::Title(title) => batch.title = Some(Some(title)),
                Event::ResetTitle => batch.title = Some(None),
                Event::ClipboardStore(kind, text) => {
                    batch.clipboard_store = Some((terminal_clipboard(kind), text));
                }
                Event::ClipboardLoad(_, formatter) => {
                    if let Some(clipboard) = clipboard() {
                        let _ = self.input(formatter(&clipboard).into_bytes());
                    }
                }
                Event::ColorRequest(index, formatter) => {
                    let color = if index == 257 {
                        alacritty_terminal::vte::ansi::Rgb {
                            r: 30,
                            g: 30,
                            b: 30,
                        }
                    } else {
                        alacritty_terminal::vte::ansi::Rgb {
                            r: 224,
                            g: 224,
                            b: 224,
                        }
                    };
                    let _ = self.input(formatter(color).into_bytes());
                }
                Event::PtyWrite(text) => {
                    let response = if text == "\x1b[?6c" {
                        log::debug!(
                            "advertising terminal Sixel support in primary device attributes"
                        );
                        "\x1b[?63;4c".as_bytes().to_vec()
                    } else {
                        text.into_bytes()
                    };
                    let _ = self.input(response);
                }
                Event::TextAreaSizeRequest(formatter) => {
                    let _ = self.input(formatter(self.viewport.window_size()).into_bytes());
                }
                Event::Bell => batch.bell = true,
                Event::Exit => {
                    self.child_exited.store(true, Ordering::Release);
                    batch.exited = true;
                    batch.needs_redraw = true;
                }
                Event::ChildExit(status) => {
                    self.child_exited.store(true, Ordering::Release);
                    batch.exited = true;
                    batch.exit_code = status.code();
                    batch.needs_redraw = true;
                }
            }
        }
        batch
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let terminal = self.terminal.lock();
        let columns = terminal.columns();
        let lines = terminal.screen_lines();
        let content = terminal.renderable_content();
        let display_offset = content.display_offset;
        let cursor = point_to_viewport(display_offset, content.cursor.point).and_then(|point| {
            (point.line < lines).then_some(TerminalCursor {
                line: point.line,
                column: point.column.0,
                shape: terminal_cursor_shape(content.cursor.shape),
            })
        });
        let images = self.sixel.snapshot(
            content.cursor.point.line.0,
            content.cursor.point.column.0,
            display_offset,
            lines,
            self.viewport.cell_height,
        );
        let mut cells = Vec::with_capacity(columns.saturating_mul(lines));
        for indexed in content.display_iter {
            let Some(point) = point_to_viewport(display_offset, indexed.point) else {
                continue;
            };
            if point.line >= lines || indexed.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let mut text = if indexed.flags.contains(Flags::HIDDEN) {
                " ".to_owned()
            } else {
                indexed.c.to_string()
            };
            if let Some(zerowidth) = indexed.zerowidth() {
                text.extend(zerowidth);
            }
            cells.push(TerminalCell {
                line: point.line,
                column: point.column.0,
                text,
                foreground: terminal_color(indexed.fg),
                background: terminal_color(indexed.bg),
                style: terminal_cell_style(indexed.flags),
                selected: content
                    .selection
                    .is_some_and(|selection| selection.contains(indexed.point)),
                hyperlink: indexed
                    .hyperlink()
                    .map(|hyperlink| hyperlink.uri().to_owned()),
            });
        }
        TerminalSnapshot {
            columns,
            lines,
            display_offset,
            cursor,
            cells,
            images,
        }
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        self.stop_io()
    }

    fn stop_io(&mut self) -> io::Result<()> {
        if self.io_thread.is_none() {
            return Ok(());
        }
        if !self.child_exited.load(Ordering::Acquire) {
            signal_process_group(self.child_pid, libc::SIGHUP);
            if !wait_for_process_group_exit(self.child_pid, Duration::from_millis(150)) {
                signal_process_group(self.child_pid, libc::SIGTERM);
            }
            if !wait_for_process_group_exit(self.child_pid, Duration::from_millis(250)) {
                signal_process_group(self.child_pid, libc::SIGKILL);
                let _ = wait_for_process_group_exit(self.child_pid, Duration::from_millis(250));
            }
        }
        let _ = self.sender.send(Msg::Shutdown);
        let join_result = self
            .io_thread
            .take()
            .map(|thread| thread.join())
            .transpose()
            .map_err(|_| io::Error::other("terminal I/O thread panicked"));
        if let Err(error) = join_result {
            return Err(error);
        }
        Ok(())
    }
}

fn terminal_search_origin(
    terminal: &Term<TerminalEventProxy>,
    direction: TerminalSearchDirection,
) -> Point {
    let selected = terminal
        .selection
        .as_ref()
        .and_then(|selection| selection.to_range(terminal));
    match (direction, selected) {
        (TerminalSearchDirection::Next, Some(range)) => terminal_point_after(terminal, range.end),
        (TerminalSearchDirection::Previous, Some(range)) => {
            terminal_point_before(terminal, range.start)
        }
        _ => terminal.renderable_content().cursor.point,
    }
}

fn terminal_point_after(terminal: &Term<TerminalEventProxy>, point: Point) -> Point {
    if point.column.0 + 1 < terminal.columns() {
        Point::new(point.line, point.column + 1)
    } else {
        Point::new(
            (point.line + Line(1)).min(terminal.bottommost_line()),
            Column(0),
        )
    }
}

fn terminal_point_before(terminal: &Term<TerminalEventProxy>, point: Point) -> Point {
    if point.column.0 > 0 {
        Point::new(point.line, point.column - 1)
    } else {
        Point::new(
            (point.line - Line(1)).max(terminal.topmost_line()),
            Column(terminal.columns().saturating_sub(1)),
        )
    }
}

fn terminal_clipboard(kind: alacritty_terminal::term::ClipboardType) -> TerminalClipboard {
    match kind {
        alacritty_terminal::term::ClipboardType::Clipboard => TerminalClipboard::Clipboard,
        alacritty_terminal::term::ClipboardType::Selection => TerminalClipboard::Selection,
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.stop_io();
    }
}

fn terminal_color(color: Color) -> TerminalColor {
    match color {
        Color::Named(color) => TerminalColor::Named(color as u16),
        Color::Indexed(color) => TerminalColor::Indexed(color),
        Color::Spec(color) => TerminalColor::Rgb(color.r, color.g, color.b),
    }
}

fn terminal_cell_style(flags: Flags) -> TerminalCellStyle {
    TerminalCellStyle {
        bold: flags.contains(Flags::BOLD),
        italic: flags.intersects(Flags::ITALIC | Flags::BOLD_ITALIC),
        dim: flags.contains(Flags::DIM),
        inverse: flags.contains(Flags::INVERSE),
        hidden: flags.contains(Flags::HIDDEN),
        strikeout: flags.contains(Flags::STRIKEOUT),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        wide: flags.contains(Flags::WIDE_CHAR),
    }
}

fn terminal_cursor_shape(shape: CursorShape) -> TerminalCursorShape {
    match shape {
        CursorShape::Block => TerminalCursorShape::Block,
        CursorShape::Underline => TerminalCursorShape::Underline,
        CursorShape::Beam => TerminalCursorShape::Beam,
        CursorShape::HollowBlock => TerminalCursorShape::HollowBlock,
        CursorShape::Hidden => TerminalCursorShape::Hidden,
    }
}

fn signal_process_group(child_pid: u32, signal: libc::c_int) {
    if let Ok(process_group) = i32::try_from(child_pid) {
        unsafe {
            libc::kill(-process_group, signal);
        }
    }
}

fn wait_for_process_group_exit(child_pid: u32, timeout: Duration) -> bool {
    let Ok(process_group) = i32::try_from(child_pid) else {
        return true;
    };
    let deadline = Instant::now() + timeout;
    loop {
        let alive = unsafe { libc::kill(-process_group, 0) } == 0
            || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
        if !alive {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}
