use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr::{NonNull, null, null_mut};
use std::slice;

use super::*;

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/ghostty.rs"));
}

use ffi::*;

fn check(result: GhosttyResult) -> io::Result<()> {
    if result == GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::other(format!("libghostty-vt returned {result}")))
    }
}

fn out<T>(value: &mut T) -> *mut c_void {
    (value as *mut T).cast()
}

fn value<T>(value: &T) -> *const c_void {
    (value as *const T).cast()
}

// Each handle is uniquely owned. Handles that refer to the terminal are
// declared before it, so they are dropped before the terminal itself.
macro_rules! handle {
    ($name:ident, $raw:ty, $free:ident) => {
        struct $name($raw);

        impl $name {
            fn new(create: impl FnOnce(*mut $raw) -> GhosttyResult) -> io::Result<Self> {
                let mut handle = null_mut();
                check(create(&mut handle))?;
                Ok(Self(handle))
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                unsafe { $free(self.0) }
            }
        }
    };
}

mod graphics;
mod sixel_compat;
use graphics::Graphics;

handle!(Terminal, GhosttyTerminal, ghostty_terminal_free);
handle!(Render, GhosttyRenderState, ghostty_render_state_free);
handle!(
    Rows,
    GhosttyRenderStateRowIterator,
    ghostty_render_state_row_iterator_free
);
handle!(
    Cells,
    GhosttyRenderStateRowCells,
    ghostty_render_state_row_cells_free
);
handle!(Formatter, GhosttyFormatter, ghostty_formatter_free);
handle!(
    TrackedCell,
    GhosttyTrackedGridRef,
    ghostty_tracked_grid_ref_free
);
handle!(KeyEncoder, GhosttyKeyEncoder, ghostty_key_encoder_free);
handle!(KeyEvent, GhosttyKeyEvent, ghostty_key_event_free);
handle!(
    MouseEncoder,
    GhosttyMouseEncoder,
    ghostty_mouse_encoder_free
);
handle!(MouseEvent, GhosttyMouseEvent, ghostty_mouse_event_free);

#[derive(Default)]
struct Effects {
    reply: Vec<u8>,
    events: TerminalEventBatch,
    reset_epoch: u64,
}

struct EffectState(NonNull<Effects>);

impl EffectState {
    fn new() -> Self {
        Self(NonNull::from(Box::leak(Box::<Effects>::default())))
    }

    fn get_mut(&mut self) -> &mut Effects {
        // The session lock excludes every Ghostty call while this borrow is live.
        unsafe { self.0.as_mut() }
    }
}

impl Drop for EffectState {
    fn drop(&mut self) {
        // The terminal and all foreign callbacks have been freed.
        unsafe { drop(Box::from_raw(self.0.as_ptr())) };
    }
}

pub(super) struct Emulator {
    graphics: Graphics,
    sixel: sixel_compat::SixelGraphics,
    rows: Rows,
    cells: Cells,
    render: Render,
    formatter: Formatter,
    selection_anchor: Option<(TrackedCell, TerminalSelectionType, TerminalSide)>,
    keys: KeyEncoder,
    key: KeyEvent,
    mouse: MouseEncoder,
    mouse_event: MouseEvent,
    terminal: Terminal,
    effects: EffectState,
    viewport: TerminalViewport,
}

// libghostty-vt instances are thread-compatible. All access to these owning
// handles and their synchronous callbacks is serialized by the session mutex.
unsafe impl Send for Emulator {}

impl Emulator {
    pub(super) fn new(viewport: TerminalViewport) -> io::Result<Self> {
        // Set global callbacks before any thread calls into Ghostty.
        let graphics = Graphics::new()?;
        unsafe {
            let terminal = Terminal::new(|out| {
                ghostty_terminal_new(null(), out, viewport.columns as u16, viewport.lines as u16)
            })?;
            let mut emulator = Self {
                graphics,
                sixel: sixel_compat::SixelGraphics::default(),
                rows: Rows::new(|out| ghostty_render_state_row_iterator_new(null(), out))?,
                cells: Cells::new(|out| ghostty_render_state_row_cells_new(null(), out))?,
                render: Render::new(|out| ghostty_render_state_new(null(), out))?,
                formatter: Formatter::new(|out| {
                    ghostty_formatter_terminal_new(null(), out, terminal.0)
                })?,
                selection_anchor: None,
                keys: KeyEncoder::new(|out| ghostty_key_encoder_new(null(), out))?,
                key: KeyEvent::new(|out| ghostty_key_event_new(null(), out))?,
                mouse: MouseEncoder::new(|out| ghostty_mouse_encoder_new(null(), out))?,
                mouse_event: MouseEvent::new(|out| ghostty_mouse_event_new(null(), out))?,
                terminal,
                effects: EffectState::new(),
                viewport,
            };
            let term = emulator.terminal.0;
            check(ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_USERDATA,
                emulator.effects.0.as_ptr().cast(),
            ))?;
            for (option, callback) in [
                (GHOSTTY_TERMINAL_OPT_WRITE_PTY, write_pty as *const c_void),
                (
                    GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                    title_changed as *const c_void,
                ),
                (GHOSTTY_TERMINAL_OPT_BELL, bell as *const c_void),
                (
                    GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
                    device_attributes as *const c_void,
                ),
                (
                    GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
                    clipboard_write as *const c_void,
                ),
                (GHOSTTY_TERMINAL_OPT_SIZE, size_report as *const c_void),
                (GHOSTTY_TERMINAL_OPT_RESET, reset as *const c_void),
            ] {
                check(ghostty_terminal_set(term, option, callback))?;
            }
            check(ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                value(&10_000usize),
            ))?;
            for (option, rgb) in [
                (
                    GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                    GhosttyColorRgb {
                        r: 224,
                        g: 224,
                        b: 224,
                    },
                ),
                (
                    GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                    GhosttyColorRgb {
                        r: 30,
                        g: 30,
                        b: 30,
                    },
                ),
            ] {
                check(ghostty_terminal_set(term, option, value(&rgb)))?;
            }
            check(ghostty_formatter_set(
                emulator.formatter.0,
                GHOSTTY_FORMATTER_OPT_UNWRAP,
                value(&true),
            ))?;
            emulator.resize(viewport)?;
            Ok(emulator)
        }
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        self.sixel.feed(self.terminal.0, bytes, &mut self.effects);
        self.effects.get_mut().events.needs_redraw = true;
    }

    pub(super) fn take_reply(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.effects.get_mut().reply)
    }

    pub(super) fn drain_events(&mut self) -> TerminalEventBatch {
        let mut events = std::mem::take(&mut self.effects.get_mut().events);
        let (changed, next_frame) = self.graphics.tick(self.terminal.0, events.needs_redraw);
        events.needs_redraw |= changed;
        events.next_animation = next_frame;
        events
    }

    pub(super) fn resize(&mut self, viewport: TerminalViewport) -> io::Result<()> {
        check(unsafe {
            ghostty_terminal_resize(
                self.terminal.0,
                viewport.columns as u16,
                viewport.lines as u16,
                viewport.cell_width.into(),
                viewport.cell_height.into(),
            )
        })?;
        self.viewport = viewport;
        self.effects.get_mut().events.needs_redraw = true;
        Ok(())
    }

    fn get<T: Default>(&self, kind: GhosttyTerminalData) -> Option<T> {
        let mut data = T::default();
        check(unsafe { ghostty_terminal_get(self.terminal.0, kind, out(&mut data)) }).ok()?;
        Some(data)
    }

    pub(super) fn scrollbar(&self) -> (usize, usize, usize) {
        let bar: GhosttyTerminalScrollbar = self
            .get(GHOSTTY_TERMINAL_DATA_SCROLLBAR)
            .unwrap_or_default();
        (bar.offset as usize, bar.len as usize, bar.total as usize)
    }

    pub(super) fn scroll(&self, scroll: TerminalScroll) {
        let (tag, delta) = match scroll {
            TerminalScroll::Lines(lines) => {
                (GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA, -(lines as isize))
            }
            TerminalScroll::PageUp => (
                GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA,
                -(self.viewport.lines as isize),
            ),
            TerminalScroll::PageDown => (
                GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA,
                self.viewport.lines as isize,
            ),
            TerminalScroll::Top => (GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP, 0),
            TerminalScroll::Bottom => (GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM, 0),
            TerminalScroll::Row(row) => {
                self.scroll_to(row);
                return;
            }
        };
        unsafe {
            ghostty_terminal_scroll_viewport(
                self.terminal.0,
                GhosttyTerminalScrollViewport {
                    tag,
                    value: GhosttyTerminalScrollViewportValue { delta },
                },
            );
        }
    }

    fn scroll_to(&self, row: usize) {
        unsafe {
            ghostty_terminal_scroll_viewport(
                self.terminal.0,
                GhosttyTerminalScrollViewport {
                    tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_ROW,
                    value: GhosttyTerminalScrollViewportValue { row },
                },
            );
        }
    }

    fn point(&self, tag: GhosttyPointTag, row: usize, col: usize) -> GhosttyPoint {
        GhosttyPoint {
            tag,
            value: GhosttyPointValue {
                coordinate: GhosttyPointCoordinate {
                    x: col.min(self.viewport.columns - 1) as u16,
                    y: row.min(u32::MAX as usize) as u32,
                },
            },
        }
    }

    fn grid_ref(&self, tag: GhosttyPointTag, row: usize, col: usize) -> Option<GhosttyGridRef> {
        let mut cell = GhosttyGridRef::default();
        check(unsafe {
            ghostty_terminal_grid_ref(self.terminal.0, self.point(tag, row, col), &mut cell)
        })
        .ok()?;
        Some(cell)
    }

    fn screen_point(&self, cell: &GhosttyGridRef) -> Option<(usize, usize)> {
        let mut point = GhosttyPointCoordinate::default();
        check(unsafe {
            ghostty_terminal_point_from_grid_ref(
                self.terminal.0,
                cell,
                GHOSTTY_POINT_TAG_SCREEN,
                &mut point,
            )
        })
        .ok()?;
        Some((point.y as usize, point.x as usize))
    }

    fn selection(&self, start: (usize, usize), end: (usize, usize)) -> Option<GhosttySelection> {
        Some(GhosttySelection {
            start: self.grid_ref(GHOSTTY_POINT_TAG_SCREEN, start.0, start.1)?,
            end: self.grid_ref(GHOSTTY_POINT_TAG_SCREEN, end.0, end.1)?,
            rectangle: false,
        })
    }

    fn set_selection(&self, selection: Option<&GhosttySelection>) {
        unsafe {
            ghostty_terminal_set(
                self.terminal.0,
                GHOSTTY_TERMINAL_OPT_SELECTION,
                selection.map_or(null(), value),
            );
        }
    }

    pub(super) fn begin_selection(
        &mut self,
        row: usize,
        col: usize,
        kind: TerminalSelectionType,
        side: TerminalSide,
    ) {
        self.clear_selection();
        let point = self.point(
            GHOSTTY_POINT_TAG_VIEWPORT,
            row.min(self.viewport.lines - 1),
            col,
        );
        self.selection_anchor = TrackedCell::new(|out| unsafe {
            ghostty_terminal_grid_ref_track(self.terminal.0, point, out)
        })
        .ok()
        .map(|anchor| (anchor, kind, side));
        if matches!(
            kind,
            TerminalSelectionType::Semantic | TerminalSelectionType::Lines
        ) {
            self.update_selection(row, col, side);
        }
    }

    pub(super) fn update_selection(&self, row: usize, col: usize, side: TerminalSide) {
        let Some((anchor, kind, anchor_side)) = &self.selection_anchor else {
            return;
        };
        let mut start = GhosttyGridRef::default();
        if unsafe { ghostty_tracked_grid_ref_snapshot(anchor.0, &mut start) } != GHOSTTY_SUCCESS {
            return;
        }
        let Some(end) = self.grid_ref(
            GHOSTTY_POINT_TAG_VIEWPORT,
            row.min(self.viewport.lines - 1),
            col,
        ) else {
            return;
        };
        let Some(start_position) = self.screen_point(&start) else {
            return;
        };
        let Some(end_position) = self.screen_point(&end) else {
            return;
        };
        let mut selection = GhosttySelection {
            start,
            end,
            rectangle: *kind == TerminalSelectionType::Block,
        };
        match kind {
            TerminalSelectionType::Simple | TerminalSelectionType::Block => {
                // Pointer positions describe edges between cells. Ghostty's
                // selection endpoints include both cells.
                let width = self.viewport.columns;
                let edge = |(row, col): (usize, usize), side| {
                    row * width + col + usize::from(side == TerminalSide::Right)
                };
                let mut a = edge(start_position, *anchor_side);
                let mut b = edge(end_position, side);
                if a == b && !selection.rectangle {
                    self.set_selection(None);
                    return;
                }
                if selection.rectangle {
                    let a_col = start_position.1 + usize::from(*anchor_side == TerminalSide::Right);
                    let b_col = end_position.1 + usize::from(side == TerminalSide::Right);
                    a = start_position.0 * width
                        + if a_col > b_col {
                            a_col - 1
                        } else {
                            a_col.min(width - 1)
                        };
                    b = end_position.0 * width
                        + if b_col > a_col {
                            b_col - 1
                        } else {
                            b_col.min(width - 1)
                        };
                } else if a < b {
                    b -= 1;
                } else {
                    a -= 1;
                }
                let Some(adjusted) = self.selection((a / width, a % width), (b / width, b % width))
                else {
                    return;
                };
                selection.start = adjusted.start;
                selection.end = adjusted.end;
            }
            TerminalSelectionType::Semantic | TerminalSelectionType::Lines => {
                let bounds = |from, to| {
                    let mut bounds = GhosttySelection::default();
                    let result = unsafe {
                        if *kind == TerminalSelectionType::Semantic {
                            ghostty_terminal_select_word_between(
                                self.terminal.0,
                                &GhosttyTerminalSelectWordBetweenOptions {
                                    size: size_of::<GhosttyTerminalSelectWordBetweenOptions>(),
                                    start: from,
                                    end: to,
                                    ..Default::default()
                                },
                                &mut bounds,
                            )
                        } else {
                            ghostty_terminal_select_line(
                                self.terminal.0,
                                &GhosttyTerminalSelectLineOptions {
                                    size: size_of::<GhosttyTerminalSelectLineOptions>(),
                                    ref_: from,
                                    ..Default::default()
                                },
                                &mut bounds,
                            )
                        }
                    };
                    (result == GHOSTTY_SUCCESS).then_some(bounds)
                };
                if let (Some(first), Some(last)) = (bounds(start, end), bounds(end, start)) {
                    (selection.start, selection.end) = if start_position <= end_position {
                        (first.start, last.end)
                    } else {
                        (first.end, last.start)
                    };
                }
            }
        }
        self.set_selection(Some(&selection));
    }

    pub(super) fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.set_selection(None);
    }

    pub(super) fn select_all(&mut self) {
        self.selection_anchor = None;
        let mut selection = GhosttySelection::default();
        let selected = unsafe { ghostty_terminal_select_all(self.terminal.0, &mut selection) }
            == GHOSTTY_SUCCESS;
        self.set_selection(selected.then_some(&selection));
    }

    fn format(&self, selection: Option<&GhosttySelection>) -> Option<String> {
        unsafe {
            check(ghostty_formatter_set(
                self.formatter.0,
                GHOSTTY_FORMATTER_OPT_SELECTION,
                selection.map_or(null(), value),
            ))
            .ok()?;
            let mut len = 0;
            let result = ghostty_formatter_format_buf(self.formatter.0, null_mut(), 0, &mut len);
            let text = match result {
                GHOSTTY_SUCCESS => Some(String::new()),
                GHOSTTY_OUT_OF_SPACE => {
                    let mut bytes = vec![0; len];
                    let result = ghostty_formatter_format_buf(
                        self.formatter.0,
                        bytes.as_mut_ptr(),
                        bytes.len(),
                        &mut len,
                    );
                    bytes.truncate(len);
                    (result == GHOSTTY_SUCCESS)
                        .then(|| String::from_utf8(bytes).ok())
                        .flatten()
                }
                _ => None,
            };
            // A formatter selection contains grid references. Clear them
            // before the terminal can mutate or free its grid pages.
            ghostty_formatter_set(self.formatter.0, GHOSTTY_FORMATTER_OPT_SELECTION, null());
            text
        }
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        self.format(Some(&self.get(GHOSTTY_TERMINAL_DATA_SELECTION)?))
    }

    pub(super) fn all_text(&self) -> String {
        self.format(None).unwrap_or_default()
    }

    pub(super) fn text_rows(&self, start: usize, end: usize) -> String {
        if start >= end {
            return String::new();
        }
        self.selection((start, 0), (end - 1, self.viewport.columns - 1))
            .and_then(|range| self.format(Some(&range)))
            .unwrap_or_default()
    }

    pub(super) fn cursor_position(&self) -> (usize, usize) {
        let row: u16 = self.get(GHOSTTY_TERMINAL_DATA_CURSOR_Y).unwrap_or_default();
        let col: u16 = self.get(GHOSTTY_TERMINAL_DATA_CURSOR_X).unwrap_or_default();
        (row.into(), col.into())
    }

    pub(super) fn history_size(&self) -> usize {
        let (_, len, total) = self.scrollbar();
        total.saturating_sub(len)
    }

    pub(super) fn search(
        &mut self,
        pattern: &str,
        direction: TerminalSearchDirection,
    ) -> Result<Option<TerminalSearchMatch>, String> {
        if pattern.is_empty() {
            self.clear_selection();
            return Ok(None);
        }
        let regex = regex::Regex::new(pattern).map_err(|error| error.to_string())?;
        let history_size = self.history_size();
        let (cursor_row, cursor_col) = self.cursor_position();
        let selection: Option<GhosttySelection> = self.get(GHOSTTY_TERMINAL_DATA_SELECTION);
        let origin = selection
            .and_then(|selection| {
                let start = self.screen_point(&selection.start)?;
                let end = self.screen_point(&selection.end)?;
                Some(match direction {
                    TerminalSearchDirection::Next => start.max(end),
                    TerminalSearchDirection::Previous => start.min(end),
                })
            })
            .unwrap_or((history_size + cursor_row, cursor_col));
        let mut text = String::new();
        let mut positions = Vec::new();
        let mut codepoints = vec![0; 8];
        let (_, _, total) = self.scrollbar();
        for row in 0..total {
            let row_start = text.len();
            for col in 0..self.viewport.columns {
                let Some(cell) = self.grid_ref(GHOSTTY_POINT_TAG_SCREEN, row, col) else {
                    break;
                };
                if is_spacer(cell_width(&cell)) {
                    continue;
                }
                positions.push((text.len(), (row, col)));
                append_cell_text(&cell, &mut codepoints, &mut text);
            }
            if !self.wrapped_row(row) {
                text.truncate(row_start + text[row_start..].trim_end_matches(' ').len());
                while positions
                    .last()
                    .is_some_and(|(index, _)| *index >= text.len())
                {
                    positions.pop();
                }
                text.push('\n');
            }
        }
        let at = |index| {
            positions
                .partition_point(|(offset, _)| *offset <= index)
                .checked_sub(1)
                .map(|i| positions[i].1)
        };
        let mut first = None;
        let mut last = None;
        let mut found = None;
        for matched in regex.find_iter(&text).filter(|m| !m.is_empty()) {
            let Some(start) = at(matched.start()) else {
                continue;
            };
            let Some(end) = at(matched.end() - 1) else {
                continue;
            };
            let range = (start, end);
            first.get_or_insert(range);
            last = Some(range);
            match direction {
                TerminalSearchDirection::Next
                    if start > origin || (selection.is_none() && start == origin) =>
                {
                    found = Some(range);
                    break;
                }
                TerminalSearchDirection::Previous
                    if end < origin || (selection.is_none() && end == origin) =>
                {
                    found = Some(range);
                }
                _ => {}
            }
        }
        let Some((start, end)) = found.or(match direction {
            TerminalSearchDirection::Next => first,
            TerminalSearchDirection::Previous => last,
        }) else {
            return Ok(None);
        };
        self.selection_anchor = None;
        self.set_selection(self.selection(start, end).as_ref());
        let (top, len, _) = self.scrollbar();
        if start.0 < top || start.0 >= top + len {
            self.scroll_to(start.0);
        }
        Ok(Some(TerminalSearchMatch {
            start_line: start.0 as i32 - history_size as i32,
            start_column: start.1,
            end_line: end.0 as i32 - history_size as i32,
            end_column: end.1,
        }))
    }

    pub(super) fn mouse_reporting_enabled(&self) -> bool {
        self.get(GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING)
            .unwrap_or(false)
    }

    fn mode(&self, number: u16) -> bool {
        let mut mode = GhosttyTerminalModeConfig {
            mode: number, // A DEC mode has the ANSI bit clear.
            value: false,
        };
        unsafe {
            ghostty_terminal_get(self.terminal.0, GHOSTTY_TERMINAL_DATA_MODE, out(&mut mode))
                == GHOSTTY_SUCCESS
                && mode.value
        }
    }

    pub(super) fn focus(&self, focused: bool) -> Vec<u8> {
        if !self.mode(1004) {
            return Vec::new();
        }
        let mut buf = vec![0; 3];
        let mut written = 0;
        let event = if focused {
            GHOSTTY_FOCUS_GAINED
        } else {
            GHOSTTY_FOCUS_LOST
        };
        unsafe { ghostty_focus_encode(event, buf.as_mut_ptr(), buf.len(), &mut written) };
        buf.truncate(written);
        buf
    }

    pub(super) fn paste(&self, text: &str) -> io::Result<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let mut text = text.as_bytes().to_vec();
        let mut buf = vec![0; text.len() + 12];
        let mut written = 0;
        check(unsafe {
            ghostty_paste_encode(
                text.as_mut_ptr().cast(),
                text.len(),
                self.mode(2004),
                buf.as_mut_ptr(),
                buf.len(),
                &mut written,
            )
        })?;
        buf.truncate(written);
        Ok(buf)
    }

    pub(super) fn key(&self, key: TerminalKeyEvent<'_>) -> io::Result<Vec<u8>> {
        unsafe {
            let event = self.key.0;
            let encoder = self.keys.0;
            check(ghostty_key_encoder_set_from_terminal(
                encoder,
                self.terminal.0,
            ))?;
            check(ghostty_key_event_set_action(
                event,
                match key.action {
                    TerminalKeyAction::Press => GHOSTTY_KEY_ACTION_PRESS,
                    TerminalKeyAction::Repeat => GHOSTTY_KEY_ACTION_REPEAT,
                    TerminalKeyAction::Release => GHOSTTY_KEY_ACTION_RELEASE,
                },
            ))?;
            check(ghostty_key_event_set_key(event, ghostty_key(key.key)))?;
            check(ghostty_key_event_set_mods(event, mods(key.modifiers)))?;
            check(ghostty_key_event_set_consumed_mods(
                event,
                mods(key.consumed_modifiers),
            ))?;
            check(ghostty_key_event_set_unshifted_codepoint(
                event,
                key.unshifted.map_or(0, u32::from),
            ))?;
            check(ghostty_key_event_set_utf8(event, string(key.text)))?;
            let mut buf = vec![0; key.text.len() + 128];
            let mut written = 0;
            let result = ghostty_key_encoder_encode(
                encoder,
                event,
                buf.as_mut_ptr(),
                buf.len(),
                &mut written,
            );
            // The event retains the UTF-8 pointer until it is replaced.
            ghostty_key_event_set_utf8(event, GhosttyString::default());
            check(result)?;
            buf.truncate(written);
            Ok(buf)
        }
    }

    pub(super) fn mouse(
        &self,
        button: TerminalMouseButton,
        action: TerminalMouseAction,
        modifiers: TerminalMouseModifiers,
        x: f32,
        y: f32,
    ) -> io::Result<Vec<u8>> {
        unsafe {
            let encoder = self.mouse.0;
            let event = self.mouse_event.0;
            check(ghostty_mouse_encoder_set_from_terminal(
                encoder,
                self.terminal.0,
            ))?;
            let width = u32::from(self.viewport.cell_width);
            let height = u32::from(self.viewport.cell_height);
            check(ghostty_mouse_encoder_set(
                encoder,
                GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                value(&GhosttyMouseEncoderSize {
                    size: size_of::<GhosttyMouseEncoderSize>(),
                    screen_width: self.viewport.columns as u32 * width,
                    screen_height: self.viewport.lines as u32 * height,
                    cell_width: width,
                    cell_height: height,
                    ..Default::default()
                }),
            ))?;
            check(ghostty_mouse_encoder_set(
                encoder,
                GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                value(&(button != TerminalMouseButton::None)),
            ))?;
            check(ghostty_mouse_event_set_action(
                event,
                match action {
                    TerminalMouseAction::Press => GHOSTTY_MOUSE_ACTION_PRESS,
                    TerminalMouseAction::Release => GHOSTTY_MOUSE_ACTION_RELEASE,
                    TerminalMouseAction::Move => GHOSTTY_MOUSE_ACTION_MOTION,
                },
            ))?;
            check(match button {
                TerminalMouseButton::None => ghostty_mouse_event_clear_button(event),
                button => ghostty_mouse_event_set_button(
                    event,
                    match button {
                        TerminalMouseButton::Left => GHOSTTY_MOUSE_BUTTON_LEFT,
                        TerminalMouseButton::Middle => GHOSTTY_MOUSE_BUTTON_MIDDLE,
                        TerminalMouseButton::Right => GHOSTTY_MOUSE_BUTTON_RIGHT,
                        TerminalMouseButton::WheelUp => GHOSTTY_MOUSE_BUTTON_FOUR,
                        TerminalMouseButton::WheelDown => GHOSTTY_MOUSE_BUTTON_FIVE,
                        TerminalMouseButton::None => unreachable!(),
                    },
                ),
            })?;
            check(ghostty_mouse_event_set_mods(
                event,
                mods(TerminalKeyModifiers {
                    shift: modifiers.shift,
                    alt: modifiers.option,
                    control: modifiers.control,
                    ..Default::default()
                }),
            ))?;
            check(ghostty_mouse_event_set_position(
                event,
                GhosttyMousePosition { x, y },
            ))?;
            let mut buf = vec![0; 64];
            let mut written = 0;
            check(ghostty_mouse_encoder_encode(
                encoder,
                event,
                buf.as_mut_ptr(),
                buf.len(),
                &mut written,
            ))?;
            buf.truncate(written);
            Ok(buf)
        }
    }

    pub(super) fn snapshot(&mut self) -> io::Result<TerminalSnapshot> {
        let mut cursor = GhosttyRenderStateCursor {
            size: size_of::<GhosttyRenderStateCursor>(),
            ..Default::default()
        };
        let mut colors = GhosttyRenderStateColors {
            size: size_of::<GhosttyRenderStateColors>(),
            ..Default::default()
        };
        let render = self.render.0;
        let mut rows = self.rows.0;
        let mut cells = self.cells.0;
        let (offset, len, total) = self.scrollbar();
        let mut snapshot = TerminalSnapshot {
            columns: self.viewport.columns,
            lines: self.viewport.lines,
            display_offset: total.saturating_sub(len + offset),
            cursor: None,
            background: TerminalColor(30, 30, 30),
            cells: Vec::new(),
            images: Vec::new(),
        };
        unsafe {
            check(ghostty_render_state_update(render, self.terminal.0))?;
            check(ghostty_render_state_get(
                render,
                GHOSTTY_RENDER_STATE_DATA_CURSOR,
                out(&mut cursor),
            ))?;
            check(ghostty_render_state_get(
                render,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                out(&mut colors),
            ))?;
            check(ghostty_render_state_get(
                render,
                GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                out(&mut rows),
            ))?;
            snapshot.background = rgb(colors.background);
            if cursor.viewport_has_value && cursor.visible {
                snapshot.cursor = Some(TerminalCursor {
                    line: cursor.viewport_y.into(),
                    column: (cursor.viewport_x
                        - u16::from(cursor.wide_tail && cursor.viewport_x > 0))
                    .into(),
                    shape: match cursor.visual_style {
                        GHOSTTY_CURSOR_STYLE_BAR => TerminalCursorShape::Beam,
                        GHOSTTY_CURSOR_STYLE_UNDERLINE => TerminalCursorShape::Underline,
                        GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW => TerminalCursorShape::HollowBlock,
                        _ => TerminalCursorShape::Block,
                    },
                });
            }
            let mut line = 0;
            let mut graphemes = vec![0; 32];
            while ghostty_render_state_row_iterator_next(rows) {
                check(ghostty_render_state_row_get(
                    rows,
                    GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    out(&mut cells),
                ))?;
                let mut selection = GhosttyRenderStateRowSelection {
                    size: size_of::<GhosttyRenderStateRowSelection>(),
                    ..Default::default()
                };
                let selected = ghostty_render_state_row_get(
                    rows,
                    GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                    out(&mut selection),
                ) == GHOSTTY_SUCCESS;
                let mut column = 0;
                while ghostty_render_state_row_cells_next(cells) {
                    let col = column;
                    column += 1;
                    let mut raw = 0;
                    check(ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                        out(&mut raw),
                    ))?;
                    let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
                    ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, out(&mut wide));
                    if wide == GHOSTTY_CELL_WIDE_SPACER_TAIL {
                        continue;
                    }
                    let selected = selected
                        && (usize::from(selection.start_x)..=usize::from(selection.end_x))
                            .contains(&col);
                    let mut has_text = false;
                    let mut styled = false;
                    let mut hyperlink = false;
                    ghostty_cell_get(raw, GHOSTTY_CELL_DATA_HAS_TEXT, out(&mut has_text));
                    ghostty_cell_get(raw, GHOSTTY_CELL_DATA_HAS_STYLING, out(&mut styled));
                    ghostty_cell_get(raw, GHOSTTY_CELL_DATA_HAS_HYPERLINK, out(&mut hyperlink));
                    let mut background = colors.background;
                    let explicit_background = ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                        out(&mut background),
                    ) == GHOSTTY_SUCCESS;
                    if !explicit_background {
                        background = colors.background;
                    }
                    if !has_text && !styled && !selected && !hyperlink && !explicit_background {
                        continue;
                    }
                    let mut foreground = colors.foreground;
                    if ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                        out(&mut foreground),
                    ) != GHOSTTY_SUCCESS
                    {
                        foreground = colors.foreground;
                    }
                    let mut style = GhosttyStyle::default();
                    if styled {
                        ghostty_render_state_row_cells_get(
                            cells,
                            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                            out(&mut style),
                        );
                    }
                    let mut text = String::new();
                    if has_text {
                        let mut buf = GhosttyBuffer {
                            ptr: graphemes.as_mut_ptr(),
                            cap: graphemes.len(),
                            len: 0,
                        };
                        let mut result = ghostty_render_state_row_cells_get(
                            cells,
                            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                            out(&mut buf),
                        );
                        if result == GHOSTTY_OUT_OF_SPACE {
                            graphemes.resize(buf.len, 0);
                            buf.ptr = graphemes.as_mut_ptr();
                            buf.cap = graphemes.len();
                            result = ghostty_render_state_row_cells_get(
                                cells,
                                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                                out(&mut buf),
                            );
                        }
                        if result == GHOSTTY_SUCCESS {
                            text = String::from_utf8_lossy(&graphemes[..buf.len]).into_owned();
                        }
                    }
                    // Virtual Kitty placements are drawn in the image plane.
                    if text.starts_with(KITTY_PLACEHOLDER) {
                        text.clear();
                    }
                    let hyperlink = hyperlink.then(|| self.hyperlink(line, col)).flatten();
                    let flags = style.flags;
                    snapshot.cells.push(TerminalCell {
                        line,
                        column: col,
                        text,
                        foreground: rgb(foreground),
                        background: rgb(background),
                        explicit_background,
                        selected,
                        hyperlink,
                        style: TerminalCellStyle {
                            // GhosttyStyleFlags, as laid out by ghostty/vt/style.h.
                            bold: flags & (1 << 0) != 0,
                            italic: flags & (1 << 1) != 0,
                            dim: flags & (1 << 2) != 0,
                            inverse: flags & (1 << 4) != 0,
                            hidden: flags & (1 << 5) != 0,
                            strikeout: flags & (1 << 6) != 0,
                            underline: flags & (7 << 8) != 0,
                            wide: wide == GHOSTTY_CELL_WIDE_WIDE,
                        },
                    });
                }
                line += 1;
            }
        }
        snapshot.images = self.graphics.render(self.terminal.0, render);
        self.sixel
            .render(self.terminal.0, offset, self.viewport, &mut snapshot.images);
        Ok(snapshot)
    }

    pub(super) fn link_at(&self, line: usize, col: usize) -> Option<LinkTarget> {
        if line >= self.viewport.lines || col >= self.viewport.columns {
            return None;
        }
        let (offset, _, total) = self.scrollbar();
        let row = offset + line;
        let tag = GHOSTTY_POINT_TAG_SCREEN;
        let mut clicked = self.grid_ref(tag, row, col)?;
        let mut clicked_column = col;
        if col > 0 && cell_width(&clicked) == GHOSTTY_CELL_WIDE_SPACER_TAIL {
            clicked_column -= 1;
            clicked = self.grid_ref(tag, row, clicked_column)?;
        }
        if let Some(uri) = hyperlink_uri(&clicked) {
            return link_target(&uri);
        }

        // Walk Ghostty's logical line so a soft wrap preserves both the URL
        // and the mapping from a clicked cell to a UTF-8 byte offset.
        let mut start = row;
        while start > 0 && self.wrapped_row(start - 1) {
            start -= 1;
        }
        let mut text = String::new();
        let mut hit = None;
        let mut codepoints = vec![0; 16];
        for current in start..total {
            for column in 0..self.viewport.columns {
                let cell = self.grid_ref(tag, current, column)?;
                if is_spacer(cell_width(&cell)) {
                    continue;
                }
                if current == row && column == clicked_column {
                    hit = Some(text.len());
                }
                append_cell_text(&cell, &mut codepoints, &mut text);
            }
            if !self.wrapped_row(current) {
                break;
            }
        }
        let hit = hit?;
        craic_language::detected_links(&text)
            .into_iter()
            .find_map(|(start, end, _)| {
                (start..end)
                    .contains(&hit)
                    .then(|| link_target(&text[start..end]))
                    .flatten()
            })
    }

    fn wrapped_row(&self, row: usize) -> bool {
        let Some(cell) = self.grid_ref(GHOSTTY_POINT_TAG_SCREEN, row, 0) else {
            return false;
        };
        let mut raw = 0;
        let mut wrapped = false;
        unsafe {
            ghostty_grid_ref_row(&cell, &mut raw) == GHOSTTY_SUCCESS
                && ghostty_row_get(raw, GHOSTTY_ROW_DATA_WRAP, out(&mut wrapped)) == GHOSTTY_SUCCESS
                && wrapped
        }
    }
    fn hyperlink(&self, line: usize, col: usize) -> Option<String> {
        hyperlink_uri(&self.grid_ref(GHOSTTY_POINT_TAG_VIEWPORT, line, col)?)
    }
}

fn hyperlink_uri(cell: &GhosttyGridRef) -> Option<String> {
    let mut len = 0;
    unsafe {
        if ghostty_grid_ref_hyperlink_uri(cell, null_mut(), 0, &mut len) != GHOSTTY_OUT_OF_SPACE {
            return None;
        }
        let mut bytes = vec![0; len];
        check(ghostty_grid_ref_hyperlink_uri(
            cell,
            bytes.as_mut_ptr(),
            bytes.len(),
            &mut len,
        ))
        .ok()?;
        bytes.truncate(len);
        String::from_utf8(bytes).ok()
    }
}

const KITTY_PLACEHOLDER: char = '\u{10eeee}';

fn is_spacer(width: GhosttyCellWide) -> bool {
    matches!(
        width,
        GHOSTTY_CELL_WIDE_SPACER_HEAD | GHOSTTY_CELL_WIDE_SPACER_TAIL
    )
}

fn append_cell_text(cell: &GhosttyGridRef, codepoints: &mut Vec<u32>, text: &mut String) {
    let mut len = 0;
    unsafe {
        let mut result =
            ghostty_grid_ref_graphemes(cell, codepoints.as_mut_ptr(), codepoints.len(), &mut len);
        if result == GHOSTTY_OUT_OF_SPACE {
            codepoints.resize(len, 0);
            result = ghostty_grid_ref_graphemes(
                cell,
                codepoints.as_mut_ptr(),
                codepoints.len(),
                &mut len,
            );
        }
        if result == GHOSTTY_SUCCESS && len != 0 && codepoints[0] != u32::from(KITTY_PLACEHOLDER) {
            text.extend(
                codepoints[..len]
                    .iter()
                    .filter_map(|&cp| char::from_u32(cp)),
            );
        } else {
            text.push(' ');
        }
    }
}

fn cell_width(cell: &GhosttyGridRef) -> GhosttyCellWide {
    let mut raw = 0;
    let mut width = GHOSTTY_CELL_WIDE_NARROW;
    unsafe {
        if ghostty_grid_ref_cell(cell, &mut raw) == GHOSTTY_SUCCESS {
            ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, out(&mut width));
        }
    }
    width
}

fn link_target(raw: &str) -> Option<LinkTarget> {
    if raw.chars().any(char::is_control) {
        return None;
    }
    if let Ok(uri) = url::Url::parse(raw) {
        match uri.scheme() {
            "https" | "http" | "mailto" => return Some(LinkTarget::Url(raw.to_owned())),
            "file" => {
                let path = uri.to_file_path().ok()?;
                let path = path.to_str()?;
                if path.chars().any(char::is_control) {
                    return None;
                }
                let location = match uri.fragment() {
                    Some(fragment) if fragment.starts_with('L') => format!("{path}#{fragment}"),
                    _ => path.to_owned(),
                };
                return match craic_language::destination_target(&location) {
                    file @ LinkTarget::File { .. } => Some(file),
                    _ => None,
                };
            }
            _ if raw.contains("://") => return None,
            _ => {} // A file location such as main.rs:42 also parses as a URL.
        }
    }
    match craic_language::destination_target(raw) {
        file @ LinkTarget::File { .. } => Some(file),
        _ => None,
    }
}

fn rgb(color: GhosttyColorRgb) -> TerminalColor {
    TerminalColor(color.r, color.g, color.b)
}

fn mods(modifiers: TerminalKeyModifiers) -> GhosttyMods {
    (if modifiers.shift {
        GHOSTTY_MODS_SHIFT
    } else {
        0
    } | if modifiers.control {
        GHOSTTY_MODS_CTRL
    } else {
        0
    } | if modifiers.alt { GHOSTTY_MODS_ALT } else { 0 }
        | if modifiers.super_key {
            GHOSTTY_MODS_SUPER
        } else {
            0
        }
        | if modifiers.caps_lock {
            GHOSTTY_MODS_CAPS_LOCK
        } else {
            0
        }
        | if modifiers.num_lock {
            GHOSTTY_MODS_NUM_LOCK
        } else {
            0
        }) as GhosttyMods
}

fn ghostty_key(key: TerminalKey) -> GhosttyKey {
    match key {
        TerminalKey::Character(ch) => match ch.to_ascii_lowercase() {
            'a'..='z' => {
                GHOSTTY_KEY_A + (ch.to_ascii_lowercase() as u32 - 'a' as u32) as GhosttyKey
            }
            '0'..='9' => GHOSTTY_KEY_DIGIT_0 + (ch as u32 - '0' as u32) as GhosttyKey,
            ' ' => GHOSTTY_KEY_SPACE,
            '\x60' => GHOSTTY_KEY_BACKQUOTE,
            '\\' => GHOSTTY_KEY_BACKSLASH,
            '[' => GHOSTTY_KEY_BRACKET_LEFT,
            ']' => GHOSTTY_KEY_BRACKET_RIGHT,
            ',' => GHOSTTY_KEY_COMMA,
            '=' => GHOSTTY_KEY_EQUAL,
            '-' => GHOSTTY_KEY_MINUS,
            '.' => GHOSTTY_KEY_PERIOD,
            '\'' => GHOSTTY_KEY_QUOTE,
            ';' => GHOSTTY_KEY_SEMICOLON,
            '/' => GHOSTTY_KEY_SLASH,
            _ => GHOSTTY_KEY_UNIDENTIFIED,
        },
        TerminalKey::Enter => GHOSTTY_KEY_ENTER,
        TerminalKey::Escape => GHOSTTY_KEY_ESCAPE,
        TerminalKey::Tab => GHOSTTY_KEY_TAB,
        TerminalKey::Backspace => GHOSTTY_KEY_BACKSPACE,
        TerminalKey::Delete => GHOSTTY_KEY_DELETE,
        TerminalKey::Insert => GHOSTTY_KEY_INSERT,
        TerminalKey::Home => GHOSTTY_KEY_HOME,
        TerminalKey::End => GHOSTTY_KEY_END,
        TerminalKey::PageUp => GHOSTTY_KEY_PAGE_UP,
        TerminalKey::PageDown => GHOSTTY_KEY_PAGE_DOWN,
        TerminalKey::Up => GHOSTTY_KEY_ARROW_UP,
        TerminalKey::Down => GHOSTTY_KEY_ARROW_DOWN,
        TerminalKey::Left => GHOSTTY_KEY_ARROW_LEFT,
        TerminalKey::Right => GHOSTTY_KEY_ARROW_RIGHT,
        TerminalKey::Function(n @ 1..=25) => GHOSTTY_KEY_F1 + GhosttyKey::from(n - 1),
        TerminalKey::Keypad(n @ 0..=9) => GHOSTTY_KEY_NUMPAD_0 + GhosttyKey::from(n),
        TerminalKey::KeypadEnter => GHOSTTY_KEY_NUMPAD_ENTER,
        TerminalKey::KeypadAdd => GHOSTTY_KEY_NUMPAD_ADD,
        TerminalKey::KeypadSubtract => GHOSTTY_KEY_NUMPAD_SUBTRACT,
        TerminalKey::KeypadMultiply => GHOSTTY_KEY_NUMPAD_MULTIPLY,
        TerminalKey::KeypadDivide => GHOSTTY_KEY_NUMPAD_DIVIDE,
        TerminalKey::KeypadDecimal => GHOSTTY_KEY_NUMPAD_DECIMAL,
        TerminalKey::KeypadEqual => GHOSTTY_KEY_NUMPAD_EQUAL,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}

fn string(text: &str) -> GhosttyString {
    GhosttyString {
        ptr: text.as_ptr(),
        len: text.len(),
    }
}

// The foreign callbacks run synchronously under the same session mutex as
// the calling Ghostty operation. Copy all borrowed data before returning.
unsafe extern "C" fn write_pty(
    _: GhosttyTerminal,
    userdata: *mut c_void,
    bytes: *const u8,
    len: usize,
) {
    if len != 0 {
        unsafe {
            (*userdata.cast::<Effects>())
                .reply
                .extend_from_slice(slice::from_raw_parts(bytes, len))
        };
    }
}

unsafe extern "C" fn title_changed(terminal: GhosttyTerminal, userdata: *mut c_void) {
    let mut title = GhosttyString::default();
    if unsafe { ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_TITLE, out(&mut title)) }
        == GHOSTTY_SUCCESS
    {
        let title = (title.len != 0).then(|| unsafe {
            String::from_utf8_lossy(slice::from_raw_parts(title.ptr, title.len)).into_owned()
        });
        let events = unsafe { &mut (*userdata.cast::<Effects>()).events };
        match title
            .as_deref()
            .and_then(|title| title.strip_prefix("craic-terminal-activity:"))
        {
            Some("active") => events.activity = Some(true),
            Some("idle") => events.activity = Some(false),
            _ => events.title = Some(title),
        }
    }
}

unsafe extern "C" fn bell(_: GhosttyTerminal, userdata: *mut c_void) {
    unsafe { (*userdata.cast::<Effects>()).events.bell = true };
}

unsafe extern "C" fn clipboard_write(
    _: GhosttyTerminal,
    userdata: *mut c_void,
    request: *const GhosttyClipboardWrite,
) {
    let request = unsafe { &*request };
    let text = if request.contents_len == 0 {
        Some(String::new())
    } else {
        unsafe { slice::from_raw_parts(request.contents, request.contents_len) }
            .iter()
            .find_map(|content| {
                let mime = unsafe { slice::from_raw_parts(content.mime.ptr, content.mime.len) };
                if !(mime == b"text/plain" || mime.starts_with(b"text/plain;")) {
                    return None;
                }
                if content.data.len == 0 {
                    return Some(String::new());
                }
                let data = unsafe { slice::from_raw_parts(content.data.ptr, content.data.len) };
                std::str::from_utf8(data).ok().map(ToOwned::to_owned)
            })
    };
    if let Some(text) = text
        && let Some(reply) = request.reply
    {
        let kind = if request.location == GHOSTTY_CLIPBOARD_LOCATION_STANDARD {
            TerminalClipboard::Clipboard
        } else {
            TerminalClipboard::Selection
        };
        unsafe {
            (*userdata.cast::<Effects>()).events.clipboard_store = Some((kind, text));
            reply(
                request,
                &GhosttyClipboardWriteReply {
                    size: size_of::<GhosttyClipboardWriteReply>(),
                    result: GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS,
                    ..Default::default()
                },
            );
        }
    }
}

unsafe extern "C" fn reset(_: GhosttyTerminal, userdata: *mut c_void) {
    let effects = unsafe { &mut *userdata.cast::<Effects>() };
    effects.reset_epoch = effects.reset_epoch.wrapping_add(1);
    effects.events.title = Some(None);
}

unsafe extern "C" fn device_attributes(
    _: GhosttyTerminal,
    _: *mut c_void,
    attrs: *mut GhosttyDeviceAttributes,
) -> bool {
    let attrs = unsafe { &mut *attrs };
    attrs.primary.conformance_level = GHOSTTY_DA_CONFORMANCE_VT220 as u16;
    attrs.primary.features[..3].copy_from_slice(&[
        GHOSTTY_DA_FEATURE_SIXEL as u16,
        GHOSTTY_DA_FEATURE_ANSI_COLOR as u16,
        GHOSTTY_DA_FEATURE_CLIPBOARD as u16,
    ]);
    attrs.primary.num_features = 3;
    attrs.secondary.device_type = GHOSTTY_DA_DEVICE_TYPE_VT220 as u16;
    true
}

unsafe extern "C" fn size_report(
    terminal: GhosttyTerminal,
    _: *mut c_void,
    size: *mut GhosttySizeReportSize,
) -> bool {
    let size = unsafe { &mut *size };
    let mut width = 0u32;
    let mut height = 0u32;
    let mut cols = 0u16;
    let mut rows = 0u16;
    unsafe {
        if ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_WIDTH_PX, out(&mut width))
            != GHOSTTY_SUCCESS
            || ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_HEIGHT_PX, out(&mut height))
                != GHOSTTY_SUCCESS
            || ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_COLS, out(&mut cols))
                != GHOSTTY_SUCCESS
            || ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_ROWS, out(&mut rows))
                != GHOSTTY_SUCCESS
            || cols == 0
            || rows == 0
        {
            return false;
        }
    }
    *size = GhosttySizeReportSize {
        rows,
        columns: cols,
        cell_width: width / u32::from(cols),
        cell_height: height / u32::from(rows),
    };
    true
}
