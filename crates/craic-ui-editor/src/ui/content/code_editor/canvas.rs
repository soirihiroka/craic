use gtk::cairo;
use gtk::pango::{FontDescription, SCALE};
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

const TEXT_WIDTH_CACHE_ENTRY_LIMIT: usize = 8192;
const TEXT_WIDTH_CACHE_BYTE_LIMIT: usize = 1024 * 1024;
const TEXT_WIDTH_CACHE_MAX_TEXT_BYTES: usize = 1024;

pub const FONT_FAMILIES: &str = "JetBrainsMono Nerd Font Mono, JetBrainsMono NFM, JetBrains Mono, monospace, Noto Sans Mono CJK SC, Noto Sans Mono CJK TC, Noto Sans Mono CJK JP, Noto Sans Mono CJK KR, Noto Color Emoji, Noto Emoji, emoji";

thread_local! {
    static FONT_DESCRIPTION_CACHE: RefCell<HashMap<i32, FontDescription>> = RefCell::new(HashMap::new());
    static FONT_METRIC_CACHE: RefCell<HashMap<i32, FontMetrics>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy)]
pub struct FontMetrics {
    pub char_width: f64,
    pub char_spacing: f64,
    pub line_height: f64,
    pub baseline_offset: f64,
}

#[derive(Clone, Copy)]
pub struct TextColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

impl TextColor {
    pub const fn rgb(red: f64, green: f64, blue: f64) -> Self {
        Self { red, green, blue }
    }
}

pub struct TextWidthCache {
    font_size: i32,
    total_bytes: usize,
    widths: HashMap<String, f64>,
    insertion_order: VecDeque<String>,
}

impl TextWidthCache {
    pub fn new(font_size: f64) -> Self {
        Self {
            font_size: font_size_key(font_size),
            total_bytes: 0,
            widths: HashMap::new(),
            insertion_order: VecDeque::new(),
        }
    }

    pub fn clear_for_font_size(&mut self, font_size: i32) {
        if self.font_size == font_size {
            return;
        }
        self.font_size = font_size;
        self.clear();
    }

    pub fn clear(&mut self) {
        self.total_bytes = 0;
        self.widths.clear();
        self.insertion_order.clear();
    }
}

pub fn measure_font_metrics(
    area: &gtk::DrawingArea,
    font_size: f64,
    fallback_line_height: impl Fn(f64) -> f64,
) -> FontMetrics {
    let font_size = font_size_key(font_size);
    if let Some(metrics) = FONT_METRIC_CACHE.with(|cache| cache.borrow().get(&font_size).copied()) {
        return metrics;
    }

    let font = font_description_for_size(font_size);
    let char_width = measure_text_width(area, &font, "0");
    let pair_width = measure_text_width(area, &font, "00");
    let layout = text_layout(area, &font, "Hg日🙂");
    let (_, height) = layout.size();
    let natural_height = (height as f64 / SCALE as f64).ceil();
    let line_height = natural_height
        .max(fallback_line_height(font_size as f64))
        .ceil();
    let baseline_offset =
        ((line_height - natural_height) / 2.0) + layout.baseline() as f64 / SCALE as f64;
    let metrics = FontMetrics {
        char_width,
        char_spacing: pair_width - (char_width * 2.0),
        line_height,
        baseline_offset,
    };
    FONT_METRIC_CACHE.with(|cache| {
        cache.borrow_mut().insert(font_size, metrics);
    });
    metrics
}

pub fn font_description(font_size: f64) -> FontDescription {
    font_description_for_size(font_size_key(font_size))
}

pub fn font_size_key(font_size: f64) -> i32 {
    font_size.round() as i32
}

pub fn cached_text_width(
    area: &gtk::DrawingArea,
    font_size: f64,
    cache: &mut TextWidthCache,
    text: &str,
) -> f64 {
    if text.is_empty() {
        return 0.0;
    }

    let font_size = font_size_key(font_size);
    let cacheable = text.len() <= TEXT_WIDTH_CACHE_MAX_TEXT_BYTES;
    if cacheable {
        cache.clear_for_font_size(font_size);
        if let Some(width) = cache.widths.get(text).copied() {
            return width;
        }
    }

    let width = measure_text_width(area, &font_description_for_size(font_size), text);
    if cacheable {
        cache_text_width(cache, text, width);
    }
    width
}

pub fn draw_plain_text(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    font_size: f64,
    text: &str,
    x: f64,
    baseline: f64,
    color: TextColor,
) {
    if text.is_empty() {
        return;
    }
    let layout = area.create_pango_layout(Some(text));
    layout.set_font_description(Some(&font_description(font_size)));
    context.move_to(x, baseline - layout.baseline() as f64 / SCALE as f64);
    context.set_source_rgb(color.red, color.green, color.blue);
    pangocairo::functions::show_layout(context, &layout);
}

fn font_description_for_size(font_size: i32) -> FontDescription {
    FONT_DESCRIPTION_CACHE.with(|cache| {
        if let Some(font) = cache.borrow().get(&font_size).cloned() {
            return font;
        }
        let mut font = FontDescription::from_string(FONT_FAMILIES);
        font.set_absolute_size(font_size as f64 * SCALE as f64);
        cache.borrow_mut().insert(font_size, font.clone());
        font
    })
}

fn cache_text_width(cache: &mut TextWidthCache, text: &str, width: f64) {
    let text_len = text.len();
    if text_len > TEXT_WIDTH_CACHE_BYTE_LIMIT || cache.widths.contains_key(text) {
        return;
    }

    while cache.widths.len() >= TEXT_WIDTH_CACHE_ENTRY_LIMIT
        || cache.total_bytes.saturating_add(text_len) > TEXT_WIDTH_CACHE_BYTE_LIMIT
    {
        let Some(oldest) = cache.insertion_order.pop_front() else {
            cache.clear();
            break;
        };
        if cache.widths.remove(&oldest).is_some() {
            cache.total_bytes = cache.total_bytes.saturating_sub(oldest.len());
        }
    }

    let key = text.to_string();
    cache.total_bytes = cache.total_bytes.saturating_add(key.len());
    cache.insertion_order.push_back(key.clone());
    cache.widths.insert(key, width);
}

fn measure_text_width(area: &gtk::DrawingArea, font: &FontDescription, text: &str) -> f64 {
    let layout = text_layout(area, font, text);
    let (width, _) = layout.size();
    width as f64 / SCALE as f64
}

fn text_layout(area: &gtk::DrawingArea, font: &FontDescription, text: &str) -> gtk::pango::Layout {
    let layout = area.create_pango_layout(Some(text));
    layout.set_font_description(Some(font));
    layout.set_single_paragraph_mode(true);
    layout
}
