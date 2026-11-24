use std::collections::VecDeque;

use crate::sixel::{SixelCapture, SixelDecoded};

use super::*;

const MAX_IMAGES: usize = 32;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct SixelGraphics {
    capture: Option<(TrackedCell, SixelCapture)>,
    awaiting_backslash: bool,
    images: VecDeque<(TrackedCell, TerminalImage)>,
    bytes: usize,
    next_id: u64,
    reset_epoch: u64,
}

impl SixelGraphics {
    pub(super) fn feed(
        &mut self,
        terminal: GhosttyTerminal,
        mut bytes: &[u8],
        effects: &mut EffectState,
    ) {
        let mut stop = GhosttyTerminalWriteStop {
            size: size_of::<GhosttyTerminalWriteStop>(),
            ..Default::default()
        };
        while !bytes.is_empty() {
            if std::mem::take(&mut self.awaiting_backslash)
                && let Some((anchor, capture)) = self.capture.take()
                && bytes[0] == b'\\'
                && let Some(image) = capture.finish()
            {
                self.store(anchor, image);
            }

            let result = unsafe {
                ghostty_terminal_vt_write_until(
                    terminal,
                    bytes.as_ptr(),
                    bytes.len(),
                    GHOSTTY_TERMINAL_BOUNDARY_DCS_START | GHOSTTY_TERMINAL_BOUNDARY_DCS_END,
                    &mut stop,
                )
            };
            if result != GHOSTTY_SUCCESS && result != GHOSTTY_NO_VALUE {
                self.capture = None;
                unsafe { ghostty_terminal_vt_write(terminal, bytes.as_ptr(), bytes.len()) };
                break;
            }
            let (consumed, remaining) = bytes.split_at(stop.consumed);
            bytes = remaining;
            let ended = stop.reached & GHOSTTY_TERMINAL_BOUNDARY_DCS_END != 0;
            if let Some((_, capture)) = &mut self.capture {
                capture.feed(&consumed[..consumed.len() - usize::from(ended)]);
            }
            if ended {
                self.awaiting_backslash = stop.end == GHOSTTY_TERMINAL_STRING_END_ST_ESC;
                if !self.awaiting_backslash {
                    self.capture = None;
                }
            }
            if self.reset_epoch != effects.get_mut().reset_epoch {
                self.reset_epoch = effects.get_mut().reset_epoch;
                self.images.clear();
                self.bytes = 0;
            }
            if stop.reached & GHOSTTY_TERMINAL_BOUNDARY_DCS_START != 0 {
                let (mut x, mut y) = (0u16, 0u16);
                unsafe {
                    ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_X, out(&mut x));
                    ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_Y, out(&mut y));
                }
                let point = GhosttyPoint {
                    tag: GHOSTTY_POINT_TAG_ACTIVE,
                    value: GhosttyPointValue {
                        coordinate: GhosttyPointCoordinate { x, y: y.into() },
                    },
                };
                self.capture = TrackedCell::new(|out| unsafe {
                    ghostty_terminal_grid_ref_track(terminal, point, out)
                })
                .ok()
                .map(|anchor| (anchor, SixelCapture::default()));
            }
        }
    }

    fn store(&mut self, anchor: TrackedCell, pixels: SixelDecoded) {
        let bytes = pixels.rgba.len();
        let info = ImageInfo::new(
            (pixels.width as i32, pixels.height as i32),
            ColorType::RGBA8888,
            AlphaType::Unpremul,
            None,
        );
        let Some(image) =
            images::raster_from_data(&info, Data::new_copy(&pixels.rgba), pixels.width * 4)
        else {
            return;
        };
        while self.images.len() >= MAX_IMAGES || self.bytes + bytes > MAX_BYTES {
            let Some((_, removed)) = self.images.pop_front() else {
                break;
            };
            self.bytes -= removed.width * removed.height * 4;
        }
        self.next_id += 1;
        self.bytes += bytes;
        self.images.push_back((
            anchor,
            TerminalImage {
                id: self.next_id,
                generation: 0,
                line: 0,
                column: 0,
                width: pixels.width,
                height: pixels.height,
                offset: (0, 0),
                source: (0, 0, pixels.width as u32, pixels.height as u32),
                layer: ImageLayer::BelowBackground,
                image,
            },
        ));
    }

    pub(super) fn render(
        &self,
        terminal: GhosttyTerminal,
        top: usize,
        viewport: TerminalViewport,
        images: &mut Vec<TerminalImage>,
    ) {
        for (anchor, image) in &self.images {
            let mut cell = GhosttyGridRef::default();
            let mut point = GhosttyPointCoordinate::default();
            // Resolve in the active screen, so an image on the primary screen
            // stays hidden while a full-screen application uses the alternate.
            unsafe {
                if ghostty_tracked_grid_ref_snapshot(anchor.0, &mut cell) != GHOSTTY_SUCCESS
                    || ghostty_terminal_point_from_grid_ref(
                        terminal,
                        &cell,
                        GHOSTTY_POINT_TAG_SCREEN,
                        &mut point,
                    ) != GHOSTTY_SUCCESS
                {
                    continue;
                }
            }
            let line = i64::from(point.y) - top as i64;
            let rows = image.height.div_ceil(usize::from(viewport.cell_height)) as i64;
            if line < viewport.lines as i64 && line + rows > 0 {
                images.push(TerminalImage {
                    line: line as i32,
                    column: i32::from(point.x),
                    ..image.clone()
                });
            }
        }
    }
}
