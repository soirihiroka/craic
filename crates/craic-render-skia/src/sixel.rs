use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_SIXEL_SEQUENCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SIXEL_DIMENSION: usize = 4096;
const MAX_SIXEL_PIXELS: usize = 4 * 1024 * 1024;
const MAX_SIXEL_REPEAT: usize = 16_384;

pub(crate) struct SixelDecoded {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub output_line: u64,
}

pub(crate) struct SixelStreamParser {
    state: StreamState,
    output_line: Arc<AtomicU64>,
}

enum StreamState {
    Ground,
    Escape,
    Dcs(DcsCapture),
}

struct DcsCapture {
    header: Vec<u8>,
    decoder: Option<SixelDecoder>,
    escape: bool,
    rejected: bool,
    bytes: usize,
}

impl SixelStreamParser {
    pub(crate) fn new(output_line: Arc<AtomicU64>) -> Self {
        Self {
            state: StreamState::Ground,
            output_line,
        }
    }

    pub(crate) fn output_line(&self) -> u64 {
        self.output_line.load(Ordering::Relaxed)
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<SixelDecoded> {
        let mut decoded = Vec::new();
        for &byte in bytes {
            let mut finish_dcs = false;
            match &mut self.state {
                StreamState::Ground => match byte {
                    b'\n' => {
                        self.output_line.fetch_add(1, Ordering::Relaxed);
                    }
                    0x1b => self.state = StreamState::Escape,
                    0x90 => self.state = StreamState::Dcs(DcsCapture::new()),
                    _ => {}
                },
                StreamState::Escape => {
                    self.state = if byte == b'P' {
                        StreamState::Dcs(DcsCapture::new())
                    } else {
                        if byte == b'\n' {
                            self.output_line.fetch_add(1, Ordering::Relaxed);
                        }
                        StreamState::Ground
                    };
                }
                StreamState::Dcs(capture) => {
                    capture.bytes = capture.bytes.saturating_add(1);
                    if capture.bytes > MAX_SIXEL_SEQUENCE_BYTES {
                        capture.rejected = true;
                        capture.decoder = None;
                    }
                    if capture.escape {
                        capture.escape = false;
                        if byte == b'\\' {
                            finish_dcs = true;
                        }
                    } else {
                        match byte {
                            0x1b => capture.escape = true,
                            0x9c => finish_dcs = true,
                            _ if capture.rejected => {}
                            _ => capture.feed(byte),
                        }
                    }
                }
            }

            if finish_dcs {
                let state = std::mem::replace(&mut self.state, StreamState::Ground);
                if let StreamState::Dcs(mut capture) = state
                    && let Some(mut image) = capture.finish()
                {
                    image.output_line = self.output_line();
                    decoded.push(image);
                }
            }
        }
        decoded
    }
}

impl DcsCapture {
    fn new() -> Self {
        Self {
            header: Vec::with_capacity(24),
            decoder: None,
            escape: false,
            rejected: false,
            bytes: 0,
        }
    }

    fn feed(&mut self, byte: u8) {
        if let Some(decoder) = self.decoder.as_mut() {
            decoder.feed(byte);
            return;
        }
        if byte == b'q' {
            self.decoder = Some(SixelDecoder::new());
        } else if (0x40..=0x7e).contains(&byte) {
            self.rejected = true;
        } else if self.header.len() < 128 {
            self.header.push(byte);
        } else {
            self.rejected = true;
        }
    }

    fn finish(&mut self) -> Option<SixelDecoded> {
        (!self.rejected)
            .then(|| self.decoder.take())
            .flatten()
            .and_then(SixelDecoder::finish)
    }
}

struct SixelDecoder {
    pixels: Vec<u8>,
    canvas_width: usize,
    canvas_height: usize,
    specified_width: usize,
    specified_height: usize,
    max_x: usize,
    max_y: usize,
    x: usize,
    y: usize,
    current_color: usize,
    palette: [[u8; 4]; 256],
    command: Option<(u8, String)>,
    rejected: bool,
}

impl SixelDecoder {
    fn new() -> Self {
        Self {
            pixels: Vec::new(),
            canvas_width: 0,
            canvas_height: 0,
            specified_width: 0,
            specified_height: 0,
            max_x: 0,
            max_y: 0,
            x: 0,
            y: 0,
            current_color: 0,
            palette: default_palette(),
            command: None,
            rejected: false,
        }
    }

    fn feed(&mut self, byte: u8) {
        if self.rejected {
            return;
        }
        if let Some((_marker, parameters)) = self.command.as_mut() {
            if byte.is_ascii_digit() || byte == b';' {
                if parameters.len() < 96 {
                    parameters.push(char::from(byte));
                } else {
                    self.rejected = true;
                }
                return;
            }
            let (marker, parameters) = self.command.take().expect("command exists");
            match marker {
                b'!' => {
                    let repeat = parameters
                        .parse::<usize>()
                        .unwrap_or(1)
                        .clamp(1, MAX_SIXEL_REPEAT);
                    if (b'?'..=b'~').contains(&byte) {
                        for _ in 0..repeat {
                            self.paint_column(byte - b'?');
                            if self.rejected {
                                break;
                            }
                        }
                    }
                    return;
                }
                b'#' => self.apply_color_command(&parameters),
                b'"' => self.apply_raster_command(&parameters),
                _ => {}
            }
        }

        match byte {
            b'!' | b'#' | b'"' => self.command = Some((byte, String::new())),
            b'$' => self.x = 0,
            b'-' => {
                self.x = 0;
                self.y = self.y.saturating_add(6);
                if self.y >= MAX_SIXEL_DIMENSION {
                    self.rejected = true;
                }
            }
            b'?'..=b'~' => self.paint_column(byte - b'?'),
            _ => {}
        }
    }

    fn paint_column(&mut self, bits: u8) {
        if self.x >= MAX_SIXEL_DIMENSION || self.y >= MAX_SIXEL_DIMENSION {
            self.rejected = true;
            return;
        }
        let required_height = self.y.saturating_add(6).min(MAX_SIXEL_DIMENSION);
        if !self.ensure_canvas(self.x + 1, required_height) {
            self.rejected = true;
            return;
        }
        let color = self.palette[self.current_color];
        for bit in 0..6 {
            if bits & (1 << bit) == 0 {
                continue;
            }
            let row = self.y + bit;
            let offset = (row * self.canvas_width + self.x) * 4;
            self.pixels[offset..offset + 4].copy_from_slice(&color);
            self.max_y = self.max_y.max(row + 1);
        }
        self.x += 1;
        self.max_x = self.max_x.max(self.x);
    }

    fn ensure_canvas(&mut self, width: usize, height: usize) -> bool {
        let width = width.max(self.specified_width).min(MAX_SIXEL_DIMENSION);
        let height = height.max(self.specified_height).min(MAX_SIXEL_DIMENSION);
        if width.saturating_mul(height) > MAX_SIXEL_PIXELS {
            return false;
        }
        if width <= self.canvas_width && height <= self.canvas_height {
            return true;
        }
        let next_width = width
            .next_power_of_two()
            .min(MAX_SIXEL_DIMENSION)
            .max(self.canvas_width);
        let next_height = height
            .next_power_of_two()
            .min(MAX_SIXEL_DIMENSION)
            .max(self.canvas_height);
        if next_width.saturating_mul(next_height) > MAX_SIXEL_PIXELS {
            return false;
        }
        let mut next = vec![0; next_width * next_height * 4];
        for row in 0..self.canvas_height {
            let source = row * self.canvas_width * 4;
            let target = row * next_width * 4;
            next[target..target + self.canvas_width * 4]
                .copy_from_slice(&self.pixels[source..source + self.canvas_width * 4]);
        }
        self.pixels = next;
        self.canvas_width = next_width;
        self.canvas_height = next_height;
        true
    }

    fn apply_color_command(&mut self, parameters: &str) {
        let values = parse_parameters(parameters);
        let Some(index) = values.first().copied() else {
            return;
        };
        self.current_color = index.min(255);
        if values.len() < 5 {
            return;
        }
        self.palette[self.current_color] = match values[1] {
            1 => hls_color(values[2], values[3], values[4]),
            2 => [
                percent_channel(values[2]),
                percent_channel(values[3]),
                percent_channel(values[4]),
                255,
            ],
            _ => self.palette[self.current_color],
        };
    }

    fn apply_raster_command(&mut self, parameters: &str) {
        let values = parse_parameters(parameters);
        if values.len() < 4 {
            return;
        }
        self.specified_width = values[2].min(MAX_SIXEL_DIMENSION);
        self.specified_height = values[3].min(MAX_SIXEL_DIMENSION);
        if self.specified_width.saturating_mul(self.specified_height) > MAX_SIXEL_PIXELS
            || !self.ensure_canvas(self.specified_width.max(1), self.specified_height.max(1))
        {
            self.rejected = true;
        }
    }

    fn finish(mut self) -> Option<SixelDecoded> {
        if self.rejected {
            return None;
        }
        let width = self.specified_width.max(self.max_x);
        let height = self.specified_height.max(self.max_y);
        if width == 0
            || height == 0
            || width > MAX_SIXEL_DIMENSION
            || height > MAX_SIXEL_DIMENSION
            || width.saturating_mul(height) > MAX_SIXEL_PIXELS
            || !self.ensure_canvas(width, height)
        {
            return None;
        }
        let mut rgba = vec![0; width * height * 4];
        for row in 0..height {
            let source = row * self.canvas_width * 4;
            let target = row * width * 4;
            rgba[target..target + width * 4]
                .copy_from_slice(&self.pixels[source..source + width * 4]);
        }
        Some(SixelDecoded {
            width,
            height,
            rgba,
            output_line: 0,
        })
    }
}

fn parse_parameters(parameters: &str) -> Vec<usize> {
    parameters
        .split(';')
        .map(|value| value.parse::<usize>().unwrap_or(0))
        .collect()
}

fn percent_channel(value: usize) -> u8 {
    ((value.min(100) * 255 + 50) / 100) as u8
}

fn hls_color(hue: usize, lightness: usize, saturation: usize) -> [u8; 4] {
    let hue = (hue % 360) as f64 / 360.0;
    let lightness = lightness.min(100) as f64 / 100.0;
    let saturation = saturation.min(100) as f64 / 100.0;
    if saturation == 0.0 {
        let gray = (lightness * 255.0).round() as u8;
        return [gray, gray, gray, 255];
    }
    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    let channel = |offset: f64| {
        let mut value = hue + offset;
        if value < 0.0 {
            value += 1.0;
        } else if value > 1.0 {
            value -= 1.0;
        }
        let value = if value < 1.0 / 6.0 {
            p + (q - p) * 6.0 * value
        } else if value < 0.5 {
            q
        } else if value < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - value) * 6.0
        } else {
            p
        };
        (value * 255.0).round() as u8
    };
    [channel(1.0 / 3.0), channel(0.0), channel(-1.0 / 3.0), 255]
}

fn default_palette() -> [[u8; 4]; 256] {
    let mut palette = [[0, 0, 0, 255]; 256];
    let ansi = [
        [0, 0, 0, 255],
        [0, 0, 205, 255],
        [205, 0, 0, 255],
        [0, 205, 0, 255],
        [205, 0, 205, 255],
        [0, 205, 205, 255],
        [205, 205, 0, 255],
        [229, 229, 229, 255],
        [127, 127, 127, 255],
        [92, 92, 255, 255],
        [255, 92, 92, 255],
        [92, 255, 92, 255],
        [255, 92, 255, 255],
        [92, 255, 255, 255],
        [255, 255, 92, 255],
        [255, 255, 255, 255],
    ];
    palette[..ansi.len()].copy_from_slice(&ansi);
    palette
}
