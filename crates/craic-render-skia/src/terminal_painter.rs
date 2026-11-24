use skia_safe::{
    Canvas, Color, Font, FontMgr, FontStyle, Paint, PaintStyle, Rect, canvas::SrcRectConstraint,
};
use std::cell::RefCell;
use std::collections::HashMap;

use crate::terminal::ImageLayer;
use crate::{
    TerminalCell, TerminalCellStyle, TerminalColor, TerminalCursorShape, TerminalSnapshot,
};

pub struct TerminalPaintRequest<'a> {
    pub snapshot: &'a TerminalSnapshot,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub font_size: f32,
    pub cursor_visible: bool,
    pub focused: bool,
    pub marked_text: Option<&'a str>,
}

pub struct TerminalPaintCache {
    font_size: f32,
    fonts: TerminalFonts,
}

impl TerminalPaintCache {
    pub fn new(font_size: f32) -> Self {
        Self {
            font_size,
            fonts: TerminalFonts::new(font_size),
        }
    }

    pub fn cell_size(&mut self, font_size: f32) -> (u16, u16) {
        let font = &self.fonts(font_size).regular;
        let (width, _) = font.measure_str("M", None);
        let (_, metrics) = font.metrics();
        (
            width.ceil().max(1.0) as u16,
            (metrics.descent - metrics.ascent + metrics.leading)
                .ceil()
                .max(1.0) as u16,
        )
    }

    fn fonts(&mut self, font_size: f32) -> &TerminalFonts {
        if self.font_size.to_bits() != font_size.to_bits() {
            self.font_size = font_size;
            self.fonts = TerminalFonts::new(font_size);
        }
        &self.fonts
    }
}

pub fn paint_terminal(
    canvas: &Canvas,
    request: TerminalPaintRequest<'_>,
    cache: &mut TerminalPaintCache,
) {
    let background = terminal_color(request.snapshot.background);
    canvas.clear(background);
    canvas.save();
    canvas.clip_rect(
        Rect::from_xywh(
            0.0,
            0.0,
            request.viewport_width.max(1.0),
            request.viewport_height.max(1.0),
        ),
        None,
        false,
    );

    let fonts = cache.fonts(request.font_size);
    let baseline =
        ((request.cell_height - request.font_size) / 2.0).max(0.0) + request.font_size * 0.82;
    let paint_images = |layer| {
        let paint = Paint::default();
        canvas.save();
        canvas.clip_rect(
            Rect::from_wh(
                request.snapshot.columns as f32 * request.cell_width,
                request.snapshot.lines as f32 * request.cell_height,
            ),
            None,
            false,
        );
        for image in request
            .snapshot
            .images
            .iter()
            .filter(|image| image.layer == layer)
        {
            let (src_x, src_y, width, height) = image.source;
            let source = Rect::from_xywh(src_x as f32, src_y as f32, width as f32, height as f32);
            let destination = Rect::from_xywh(
                image.column as f32 * request.cell_width + image.offset.0 as f32,
                image.line as f32 * request.cell_height + image.offset.1 as f32,
                image.width as f32,
                image.height as f32,
            );
            canvas.draw_image_rect(
                &image.image,
                Some((&source, SrcRectConstraint::Strict)),
                destination,
                &paint,
            );
        }
        canvas.restore();
    };
    paint_images(ImageLayer::BelowBackground);
    for cell in &request.snapshot.cells {
        let (_, cell_background) = cell_colors(cell);
        if cell.explicit_background || cell_background != background {
            let mut paint = Paint::default();
            paint.set_color(cell_background);
            canvas.draw_rect(
                Rect::from_xywh(
                    cell.column as f32 * request.cell_width,
                    cell.line as f32 * request.cell_height,
                    if cell.style.wide {
                        request.cell_width * 2.0
                    } else {
                        request.cell_width
                    },
                    request.cell_height,
                ),
                &paint,
            );
        }
    }
    paint_images(ImageLayer::BelowText);
    for cell in &request.snapshot.cells {
        paint_cell(
            canvas,
            fonts,
            cell,
            request.cell_width,
            request.cell_height,
            baseline,
        );
    }
    paint_images(ImageLayer::AboveText);

    if request.cursor_visible
        && let Some(cursor) = request.snapshot.cursor
        && cursor.shape != TerminalCursorShape::Hidden
    {
        let x = cursor.column as f32 * request.cell_width;
        let y = cursor.line as f32 * request.cell_height;
        let mut paint = Paint::default();
        paint.set_color(if request.focused {
            Color::from_argb(150, 230, 230, 230)
        } else {
            Color::from_argb(180, 125, 125, 125)
        });
        match cursor.shape {
            TerminalCursorShape::Block => {
                canvas.draw_rect(
                    Rect::from_xywh(x, y, request.cell_width, request.cell_height),
                    &paint,
                );
            }
            TerminalCursorShape::Underline => {
                canvas.draw_rect(
                    Rect::from_xywh(x, y + request.cell_height - 2.0, request.cell_width, 2.0),
                    &paint,
                );
            }
            TerminalCursorShape::Beam => {
                canvas.draw_rect(Rect::from_xywh(x, y, 2.0, request.cell_height), &paint);
            }
            TerminalCursorShape::HollowBlock => {
                paint.set_style(PaintStyle::Stroke);
                paint.set_stroke_width(1.0);
                canvas.draw_rect(
                    Rect::from_xywh(
                        x + 0.5,
                        y + 0.5,
                        (request.cell_width - 1.0).max(1.0),
                        (request.cell_height - 1.0).max(1.0),
                    ),
                    &paint,
                );
            }
            TerminalCursorShape::Hidden => {}
        }
    }
    if let Some(marked_text) = request.marked_text.filter(|text| !text.is_empty())
        && let Some(cursor) = request.snapshot.cursor
    {
        let x = cursor.column as f32 * request.cell_width;
        let y = cursor.line as f32 * request.cell_height;
        let mut background = Paint::default();
        background.set_color(Color::from_rgb(62, 62, 65));
        canvas.draw_rect(
            Rect::from_xywh(
                x,
                y,
                request.cell_width * marked_text.chars().count().max(1) as f32,
                request.cell_height,
            ),
            &background,
        );
        let mut foreground = Paint::default();
        foreground.set_color(Color::WHITE);
        let marked_width = request.cell_width * marked_text.chars().count().max(1) as f32;
        let marked_font = fonts.for_text(TerminalCellStyle::default(), marked_text, marked_width);
        canvas.draw_str(
            marked_text,
            (x, y + baseline),
            marked_font.as_font(),
            &foreground,
        );
        let mut underline = Paint::default();
        underline.set_color(Color::from_rgb(145, 180, 255));
        canvas.draw_rect(
            Rect::from_xywh(
                x,
                y + request.cell_height - 2.0,
                request.cell_width * marked_text.chars().count().max(1) as f32,
                1.0,
            ),
            &underline,
        );
    }
    canvas.restore();
}

fn paint_cell(
    canvas: &Canvas,
    fonts: &TerminalFonts,
    cell: &TerminalCell,
    cell_width: f32,
    cell_height: f32,
    baseline: f32,
) {
    let x = cell.column as f32 * cell_width;
    let y = cell.line as f32 * cell_height;
    let (foreground, _) = cell_colors(cell);
    if !cell.style.hidden && !cell.text.trim_matches([' ', '\t']).is_empty() {
        let mut paint = Paint::default();
        paint.set_color(foreground);
        let available_width = if cell.style.wide {
            cell_width * 2.0
        } else {
            cell_width
        };
        let font = fonts.for_text(cell.style, &cell.text, available_width);
        canvas.draw_str(&cell.text, (x, y + baseline), font.as_font(), &paint);
    }
    if cell.style.underline || cell.hyperlink.is_some() {
        let mut paint = Paint::default();
        paint.set_color(foreground);
        canvas.draw_rect(
            Rect::from_xywh(x, y + cell_height - 2.0, cell_width, 1.0),
            &paint,
        );
    }
    if cell.style.strikeout {
        let mut paint = Paint::default();
        paint.set_color(foreground);
        canvas.draw_rect(
            Rect::from_xywh(x, y + cell_height * 0.52, cell_width, 1.0),
            &paint,
        );
    }
}

struct TerminalFonts {
    manager: FontMgr,
    regular: Font,
    bold: Font,
    italic: Font,
    bold_italic: Font,
    fallback: RefCell<HashMap<(bool, bool, String, u32), Option<Font>>>,
}

impl TerminalFonts {
    fn new(size: f32) -> Self {
        let manager = FontMgr::default();
        let font = |style| {
            Font::new(
                manager
                    .match_family_style(
                        if cfg!(target_os = "macos") {
                            "Menlo"
                        } else {
                            "monospace"
                        },
                        style,
                    )
                    .or_else(|| manager.legacy_make_typeface(None, style))
                    .expect("Skia must provide a terminal typeface"),
                size,
            )
        };
        Self {
            regular: font(FontStyle::normal()),
            bold: font(FontStyle::bold()),
            italic: font(FontStyle::italic()),
            bold_italic: font(FontStyle::bold_italic()),
            manager,
            fallback: RefCell::new(HashMap::new()),
        }
    }

    fn for_style(&self, style: TerminalCellStyle) -> &Font {
        match (style.bold, style.italic) {
            (true, true) => &self.bold_italic,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (false, false) => &self.regular,
        }
    }

    fn for_text(
        &self,
        style: TerminalCellStyle,
        text: &str,
        available_width: f32,
    ) -> ResolvedTerminalFont<'_> {
        let primary = self.for_style(style);
        let Some(missing) = text
            .chars()
            .find(|character| primary.unichar_to_glyph(*character as i32) == 0)
        else {
            return ResolvedTerminalFont::Borrowed(primary);
        };
        let key = (
            style.bold,
            style.italic,
            text.to_owned(),
            available_width.to_bits(),
        );
        if let Some(cached) = self.fallback.borrow().get(&key) {
            return match cached {
                Some(font) => ResolvedTerminalFont::Owned(font.clone()),
                None => ResolvedTerminalFont::Borrowed(primary),
            };
        }
        let font_style = match (style.bold, style.italic) {
            (true, true) => FontStyle::bold_italic(),
            (true, false) => FontStyle::bold(),
            (false, true) => FontStyle::italic(),
            (false, false) => FontStyle::normal(),
        };
        let manager = &self.manager;
        let typeface = [
            "JetBrainsMono Nerd Font Mono",
            "Symbols Nerd Font Mono",
            "MesloLGS NF",
            "Hack Nerd Font Mono",
        ]
        .into_iter()
        .find_map(|family| {
            manager
                .match_family_style_character(family, font_style, &[], missing as i32)
                .filter(|typeface| {
                    let font = Font::new(typeface.clone(), primary.size());
                    text.chars()
                        .all(|character| font.unichar_to_glyph(character as i32) != 0)
                })
        })
        .or_else(|| manager.match_family_style_character("", font_style, &[], missing as i32));
        let fallback = typeface.map(|typeface| {
            let mut font = Font::new(typeface, primary.size());
            let (text_width, _) = font.measure_str(text, None);
            if text_width > available_width && available_width > 0.0 {
                font.set_scale_x((available_width / text_width).clamp(0.55, 1.0));
            }
            font
        });
        let mut cache = self.fallback.borrow_mut();
        if cache.len() >= 2048 {
            cache.clear();
        }
        cache.insert(key, fallback.clone());
        match fallback {
            Some(font) => ResolvedTerminalFont::Owned(font),
            None => ResolvedTerminalFont::Borrowed(primary),
        }
    }
}

enum ResolvedTerminalFont<'a> {
    Borrowed(&'a Font),
    Owned(Font),
}

impl ResolvedTerminalFont<'_> {
    fn as_font(&self) -> &Font {
        match self {
            Self::Borrowed(font) => font,
            Self::Owned(font) => font,
        }
    }
}

fn terminal_color(TerminalColor(red, green, blue): TerminalColor) -> Color {
    Color::from_rgb(red, green, blue)
}

fn cell_colors(cell: &TerminalCell) -> (Color, Color) {
    if cell.selected {
        return (Color::WHITE, Color::from_rgb(46, 95, 145));
    }
    let (mut fg, mut bg) = (
        terminal_color(cell.foreground),
        terminal_color(cell.background),
    );
    if cell.style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.style.dim {
        fg = dim_color(fg);
    }
    (fg, bg)
}

fn dim_color(color: Color) -> Color {
    Color::from_argb(
        color.a(),
        ((color.r() as u16 * 2) / 3) as u8,
        ((color.g() as u16 * 2) / 3) as u8,
        ((color.b() as u16 * 2) / 3) as u8,
    )
}
