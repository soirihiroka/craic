use super::super::skia_canvas;
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextStyle, TypefaceFontProvider,
};
use skia_safe::{Color, FontMgr, FontStyle};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;

const TEXT_WIDTH_CACHE_ENTRY_LIMIT: usize = 8192;
const TEXT_WIDTH_CACHE_BYTE_LIMIT: usize = 1024 * 1024;
const TEXT_WIDTH_CACHE_MAX_TEXT_BYTES: usize = 1024;
const TAB_REPLACEMENT: &str = "    ";
const EDITOR_FONT_FAMILY: &str = "Craic Editor Mono";
const FONT_FAMILIES: &[&str] = &[
    EDITOR_FONT_FAMILY,
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono NFM",
    "JetBrains Mono",
    "monospace",
    "Noto Sans Mono CJK SC",
    "Noto Sans Mono CJK TC",
    "Noto Sans Mono CJK JP",
    "Noto Sans Mono CJK KR",
    "Noto Color Emoji",
    "Noto Emoji",
    "emoji",
];

thread_local! {
    static FONT_COLLECTION: FontCollection = build_font_collection();
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
    pub alpha: f64,
}

impl TextColor {
    pub const fn rgb(red: f64, green: f64, blue: f64) -> Self {
        Self::rgba(red, green, blue, 1.0)
    }

    pub const fn rgba(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
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
    _area: &gtk::GLArea,
    font_size: f64,
    fallback_line_height: impl Fn(f64) -> f64,
) -> FontMetrics {
    let font_size = font_size_key(font_size);
    if let Some(metrics) = FONT_METRIC_CACHE.with(|cache| cache.borrow().get(&font_size).copied()) {
        return metrics;
    }

    let char_width = measure_text_width(font_size, "0");
    let pair_width = measure_text_width(font_size, "00");
    let paragraph = paragraph(font_size, "Hg日🙂", TextColor::rgb(0.0, 0.0, 0.0));
    let natural_height = paragraph.height().ceil() as f64;
    let line_height = natural_height
        .max(fallback_line_height(font_size as f64))
        .ceil();
    let baseline_offset =
        ((line_height - natural_height) / 2.0) + paragraph.alphabetic_baseline() as f64;
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

pub fn font_size_key(font_size: f64) -> i32 {
    font_size.round() as i32
}

pub fn cached_text_width(
    _area: &gtk::GLArea,
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

    let width = measure_text_width(font_size, text);
    if cacheable {
        cache_text_width(cache, text, width);
    }
    width
}

pub fn draw_plain_text(
    _area: &gtk::GLArea,
    context: &skia_canvas::Context<'_>,
    font_size: f64,
    text: &str,
    x: f64,
    baseline: f64,
    color: TextColor,
) {
    if text.is_empty() {
        return;
    }
    let paragraph = paragraph(font_size_key(font_size), text, color);
    paragraph.paint(
        context.canvas(),
        (
            x as f32,
            (baseline - paragraph.alphabetic_baseline() as f64) as f32,
        ),
    );
}

fn paragraph(font_size: i32, text: &str, color: TextColor) -> Paragraph {
    FONT_COLLECTION.with(|fonts| {
        let text = if text.contains('\t') {
            Cow::Owned(text.replace('\t', TAB_REPLACEMENT))
        } else {
            Cow::Borrowed(text)
        };
        let mut text_style = TextStyle::new();
        text_style
            .set_color(Color::from_argb(
                channel(color.alpha),
                channel(color.red),
                channel(color.green),
                channel(color.blue),
            ))
            .set_font_size(font_size as f32)
            .set_font_style(FontStyle::normal())
            .set_font_families(FONT_FAMILIES);
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_style(&text_style);
        let mut builder = ParagraphBuilder::new(&paragraph_style, fonts.clone());
        builder.push_style(&text_style);
        builder.add_text(&text);
        builder.pop();
        let mut paragraph = builder.build();
        paragraph.layout(1_000_000.0);
        paragraph
    })
}

fn measure_text_width(font_size: i32, text: &str) -> f64 {
    paragraph(font_size, text, TextColor::rgb(0.0, 0.0, 0.0)).max_intrinsic_width() as f64
}

fn channel(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn build_font_collection() -> FontCollection {
    let system_manager = FontMgr::new();
    let mut bundled_manager = TypefaceFontProvider::new();
    let mut loaded = 0;
    match fs::read_dir(craic_ui_core::ui::bundled_font_dir()) {
        Ok(entries) => {
            for path in entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| matches!(extension, "ttf" | "otf"))
            }) {
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                if let Some(typeface) = system_manager.new_from_data(&bytes, None) {
                    bundled_manager.register_typeface(typeface, Some(EDITOR_FONT_FAMILY));
                    loaded += 1;
                }
            }
        }
        Err(error) => log::warn!("skia text could not read bundled font directory: {error}"),
    }

    let bundled_monospace_ready = bundled_manager
        .match_family_style(EDITOR_FONT_FAMILY, FontStyle::normal())
        .is_some();
    let mut collection = FontCollection::new();
    if loaded > 0 {
        collection.set_asset_font_manager(Some(FontMgr::from(bundled_manager)));
    }
    collection.set_default_font_manager(system_manager, Some("monospace"));
    collection.enable_font_fallback();
    log::debug!(
        "skia text initialized bundled_typefaces={loaded} monospace_alias={bundled_monospace_ready} system_fallback=true"
    );
    collection
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
