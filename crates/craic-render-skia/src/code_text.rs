use std::borrow::Cow;

use skia_safe::{Canvas, Color, Font, FontMgr, FontStyle, Paint};

pub struct CodeTextPaintCache {
    font: Font,
    font_size_bits: u32,
}

impl CodeTextPaintCache {
    pub fn new(font_size: f32) -> Self {
        Self {
            font: monospace_font(font_size),
            font_size_bits: font_size.to_bits(),
        }
    }

    pub(crate) fn draw(
        &mut self,
        canvas: &Canvas,
        text: &str,
        x: f32,
        baseline: f32,
        color: Color,
        font_size: f32,
        align_right: bool,
    ) {
        if text.is_empty() {
            return;
        }
        if self.font_size_bits != font_size.to_bits() {
            self.font = monospace_font(font_size);
            self.font_size_bits = font_size.to_bits();
        }
        let text = if text.contains('\t') {
            Cow::Owned(text.replace('\t', "    "))
        } else {
            Cow::Borrowed(text)
        };
        let x = if align_right {
            x - self.font.measure_str(text.as_ref(), None).0
        } else {
            x
        };
        let mut paint = Paint::default();
        paint.set_color(color);
        canvas.draw_str(text.as_ref(), (x, baseline), &self.font, &paint);
    }
}

fn monospace_font(size: f32) -> Font {
    let manager = FontMgr::default();
    let style = FontStyle::normal();
    let typeface = ["SF Mono", "Menlo", "monospace"]
        .into_iter()
        .find_map(|family| manager.match_family_style(family, style))
        .or_else(|| manager.legacy_make_typeface(None, style))
        .expect("Skia must provide a monospace typeface");
    Font::new(typeface, size)
}
