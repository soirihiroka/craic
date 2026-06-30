use crate::git::BytesComparison;
use crate::ui::content::pdf_preview;
use crate::ui::{file_type, widgets};
use adw::prelude::*;
use gtk::{cairo, gdk};
use std::cell::RefCell;
use std::rc::Rc;
use webkit6::prelude::*;

pub(in crate::ui) struct BinaryPreviewWidgets {
    pub(in crate::ui) root: gtk::Box,
    title: gtk::Label,
    before: BinaryPreviewPane,
    after: BinaryPreviewPane,
    message: gtk::Label,
}

const FONT_PREVIEW_MARGIN: f64 = 24.0;
const FONT_SECTION_SPACING: f64 = 16.0;
const FONT_LINE_SPACING: f64 = 2.0;
const FONT_LOWERCASE_TEXT: &str = "abcdefghijklmnopqrstuvwxyz";
const FONT_UPPERCASE_TEXT: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const FONT_PUNCTUATION_TEXT: &str = "0123456789.:,;(*!?')";
const FONT_SAMPLE_TEXT: &str = "Sphinx of black quartz, judge my vow.";
const FREETYPE_ENCODING_UNICODE: u32 = 1_970_170_211;
const FONT_SAMPLE_SIZES: [f64; 14] = [
    8.0, 10.0, 12.0, 18.0, 24.0, 36.0, 48.0, 72.0, 96.0, 120.0, 144.0, 168.0, 192.0, 216.0,
];

struct BinaryPreviewPane {
    root: gtk::Box,
    title: gtk::Label,
    picture: gtk::Picture,
    web_view: webkit6::WebView,
    font_area: gtk::DrawingArea,
    font_preview: Rc<RefCell<Option<FontPreviewFace>>>,
    pdf_preview: pdf_preview::PdfPreview,
    empty: gtk::Label,
}

struct FontPreviewFace {
    font_face: cairo::FontFace,
    face: freetype::face::Face,
    font_name: String,
    lowercase_text: Option<&'static str>,
    uppercase_text: Option<&'static str>,
    punctuation_text: Option<&'static str>,
    sample_string: Option<&'static str>,
}

impl BinaryPreviewWidgets {
    pub(in crate::ui) fn new(title: &str) -> Self {
        let title = widgets::heading(title);
        title.set_margin_top(8);
        title.set_margin_start(12);
        title.set_margin_end(12);

        let message = widgets::muted("");
        message.set_margin_start(12);
        message.set_margin_end(12);

        let before = BinaryPreviewPane::new("Before");
        let after = BinaryPreviewPane::new("After");
        let previews = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .vexpand(true)
            .build();
        previews.append(&before.root);
        previews.append(&after.root);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&title);
        root.append(&message);
        root.append(&previews);

        Self {
            root,
            title,
            before,
            after,
            message,
        }
    }

    pub(in crate::ui) fn set_font_single(&self, file_path: &str, bytes: &[u8]) {
        self.title.set_visible(false);
        self.message.set_visible(false);
        self.before.root.set_visible(true);
        self.after.root.set_visible(false);
        self.before.title.set_label("");
        set_preview_pane(
            &self.before,
            file_path,
            Some(bytes),
            "No font",
            set_font_pane,
        );
    }

    pub(in crate::ui) fn set_pdf_single(&self, file_path: &str, bytes: &[u8]) {
        self.title.set_visible(false);
        self.message.set_visible(false);
        self.before.root.set_visible(true);
        self.after.root.set_visible(false);
        self.before.title.set_visible(false);
        set_preview_pane(&self.before, file_path, Some(bytes), "No PDF", set_pdf_pane);
    }
}

impl BinaryPreviewPane {
    fn new(title: &str) -> Self {
        let title = widgets::muted(title);
        let picture = gtk::Picture::builder()
            .hexpand(true)
            .vexpand(true)
            .can_shrink(true)
            .build();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_visible(false);

        let web_view = webkit6::WebView::new();
        web_view.set_hexpand(true);
        web_view.set_vexpand(true);

        let font_preview: Rc<RefCell<Option<FontPreviewFace>>> = Rc::new(RefCell::new(None));
        let font_preview_for_draw = Rc::clone(&font_preview);
        let pdf_preview = pdf_preview::PdfPreview::new();

        let font_area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        font_area.set_draw_func({
            let font_preview = font_preview_for_draw;
            move |area, context, width, height| {
                let font_preview = font_preview.borrow();
                let Some(font_preview) = font_preview.as_ref() else {
                    return;
                };
                render_font_sample(area, context, width, height, font_preview);
            }
        });
        font_area.set_visible(false);

        let empty = widgets::muted("No preview");
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);
        web_view.set_visible(false);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&title);
        root.append(&picture);
        root.append(&web_view);
        root.append(&pdf_preview.root);
        root.append(&font_area);
        root.append(&empty);

        Self {
            root,
            title,
            picture,
            web_view,
            font_area,
            font_preview,
            pdf_preview,
            empty,
        }
    }
}

pub(in crate::ui) fn set_image_preview(
    preview: &BinaryPreviewWidgets,
    file_path: &str,
    comparison: &BytesComparison,
) {
    set_comparison_preview(
        preview,
        file_path,
        comparison.before.as_deref(),
        comparison.after.as_deref(),
        true,
        "No image",
        set_image_pane,
    );
}

pub(in crate::ui) fn set_audio_preview(
    preview: &BinaryPreviewWidgets,
    file_path: &str,
    comparison: &BytesComparison,
) {
    set_comparison_preview(
        preview,
        file_path,
        comparison.before.as_deref(),
        comparison.after.as_deref(),
        true,
        "No audio",
        set_audio_pane,
    );
}

pub(in crate::ui) fn set_video_preview(
    preview: &BinaryPreviewWidgets,
    file_path: &str,
    comparison: &BytesComparison,
) {
    set_comparison_preview(
        preview,
        file_path,
        comparison.before.as_deref(),
        comparison.after.as_deref(),
        true,
        "No video",
        set_video_pane,
    );
}

pub(in crate::ui) fn set_font_preview(
    preview: &BinaryPreviewWidgets,
    file_path: &str,
    comparison: &BytesComparison,
) {
    set_comparison_preview(
        preview,
        file_path,
        comparison.before.as_deref(),
        comparison.after.as_deref(),
        false,
        "No font",
        set_font_pane,
    );
}

pub(in crate::ui) fn set_pdf_preview(
    preview: &BinaryPreviewWidgets,
    file_path: &str,
    comparison: &BytesComparison,
) {
    set_comparison_preview(
        preview,
        file_path,
        comparison.before.as_deref(),
        comparison.after.as_deref(),
        false,
        "No PDF",
        set_pdf_pane,
    );
}

pub(in crate::ui) fn set_unavailable_preview(
    preview: &BinaryPreviewWidgets,
    file_path: &str,
    message: &str,
) {
    preview.title.set_label(file_path);
    preview.title.set_visible(true);
    preview.message.set_label(message);
    preview.message.set_visible(true);
    preview.before.root.set_visible(true);
    preview.after.root.set_visible(false);
    preview.before.title.set_label("");
    clear_pane(&preview.before);
    preview.before.empty.set_visible(false);
}

type PaneRenderer = fn(&BinaryPreviewPane, &str, &[u8]);

fn set_comparison_preview(
    preview: &BinaryPreviewWidgets,
    file_path: &str,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
    show_title: bool,
    empty_label: &'static str,
    render: PaneRenderer,
) {
    preview.title.set_visible(show_title);
    if show_title {
        preview.title.set_label(file_path);
    }
    preview.message.set_visible(false);

    match (before, after) {
        (None, Some(after)) => {
            preview.before.root.set_visible(false);
            clear_pane(&preview.before);
            preview.after.root.set_visible(true);
            preview.after.title.set_label("Added");
            set_preview_pane(&preview.after, file_path, Some(after), empty_label, render);
        }
        (Some(before), None) => {
            preview.before.root.set_visible(true);
            preview.after.root.set_visible(false);
            clear_pane(&preview.after);
            preview.before.title.set_label("Deleted");
            set_preview_pane(
                &preview.before,
                file_path,
                Some(before),
                empty_label,
                render,
            );
        }
        _ => {
            preview.before.root.set_visible(true);
            preview.after.root.set_visible(true);
            preview.before.title.set_label("Before");
            preview.after.title.set_label("After");
            set_preview_pane(&preview.before, file_path, before, empty_label, render);
            set_preview_pane(&preview.after, file_path, after, empty_label, render);
        }
    }
}

fn set_preview_pane(
    pane: &BinaryPreviewPane,
    file_path: &str,
    bytes: Option<&[u8]>,
    empty_label: &'static str,
    render: PaneRenderer,
) {
    let Some(bytes) = bytes else {
        clear_pane(pane);
        pane.empty.set_label(empty_label);
        pane.empty.set_visible(true);
        return;
    };

    render(pane, file_path, bytes);
}

fn set_image_pane(pane: &BinaryPreviewPane, _file_path: &str, bytes: &[u8]) {
    clear_pane(pane);
    let bytes = gtk::glib::Bytes::from(bytes);
    match gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => {
            pane.picture.set_paintable(Some(&texture));
            pane.picture.set_visible(true);
            pane.empty.set_visible(false);
        }
        Err(_) => {
            pane.empty.set_label("Unable to load image");
            pane.empty.set_visible(true);
        }
    }
}

fn set_audio_pane(pane: &BinaryPreviewPane, file_path: &str, bytes: &[u8]) {
    set_media_pane(pane, file_path, "audio", bytes);
}

fn set_video_pane(pane: &BinaryPreviewPane, file_path: &str, bytes: &[u8]) {
    set_media_pane(pane, file_path, "video", bytes);
}

fn set_media_pane(pane: &BinaryPreviewPane, file_path: &str, tag: &str, bytes: &[u8]) {
    clear_pane(pane);
    pane.web_view.load_html(
        &media_preview_html(file_path, tag, bytes),
        Some("about:blank"),
    );
    pane.web_view.set_visible(true);
    pane.empty.set_visible(false);
}

fn media_preview_html(file_path: &str, tag: &str, bytes: &[u8]) -> String {
    let mime = file_type::detect(file_path, false).mime;
    let data_url = data_url(mime, bytes);
    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>
:root {{
  color-scheme: light dark;
  font-family: Cantarell, sans-serif;
}}
body {{
  display: grid;
  min-height: 100vh;
  margin: 0;
  padding: 24px;
  box-sizing: border-box;
  background: transparent;
  color: CanvasText;
}}
.frame {{
  display: grid;
  place-items: center;
  width: 100%;
  min-height: 100%;
}}
audio {{
  width: min(100%, 1000px);
  min-height: 64px;
}}
video {{
  width: 100%;
  height: 100%;
  object-fit: contain;
}}
</style>
</head>
<body>
<div class="frame"><{tag} controls src="{data_url}"></{tag}></div>
</body>
</html>"#
    )
}

fn set_font_pane(pane: &BinaryPreviewPane, file_path: &str, bytes: &[u8]) {
    let Some(font_preview) = FontPreviewFace::new(bytes) else {
        clear_pane(pane);
        pane.empty.set_label("No font");
        pane.empty.set_visible(true);
        log::warn!("Failed to load font preview for {file_path}");
        return;
    };

    clear_pane(pane);
    *pane.font_preview.borrow_mut() = Some(font_preview);
    pane.font_area.set_visible(true);
    pane.empty.set_visible(false);
    pane.font_area.queue_draw();
}

fn set_pdf_pane(pane: &BinaryPreviewPane, file_path: &str, bytes: &[u8]) {
    pane.title.set_visible(false);
    clear_non_pdf_pane(pane);
    pane.empty.set_visible(false);
    pane.pdf_preview.set_pdf(file_path, bytes);
}

impl FontPreviewFace {
    fn new(bytes: &[u8]) -> Option<Self> {
        let library = freetype::Library::init().ok()?;
        let face = library.new_memory_face(bytes.to_vec(), 0).ok()?;
        select_best_charmap(&face);
        let font_face = cairo::FontFace::create_from_ft(&face).ok()?;
        let font_name = font_name(&face);
        let lowercase_text =
            font_contains_text(&face, FONT_LOWERCASE_TEXT).then_some(FONT_LOWERCASE_TEXT);
        let uppercase_text =
            font_contains_text(&face, FONT_UPPERCASE_TEXT).then_some(FONT_UPPERCASE_TEXT);
        let punctuation_text =
            font_contains_text(&face, FONT_PUNCTUATION_TEXT).then_some(FONT_PUNCTUATION_TEXT);
        let sample_string = font_contains_text(&face, FONT_SAMPLE_TEXT).then_some(FONT_SAMPLE_TEXT);

        Some(Self {
            font_face,
            face,
            font_name,
            lowercase_text,
            uppercase_text,
            punctuation_text,
            sample_string,
        })
    }
}

fn select_best_charmap(face: &freetype::face::Face) {
    if select_charmap_by_encoding(face, FREETYPE_ENCODING_UNICODE) {
        return;
    }

    for index in 0..face.num_charmaps() {
        let charmap = face.get_charmap(index as isize);
        if face.set_charmap(&charmap).is_ok() && face.chars().next().is_some() {
            break;
        }
    }
}

fn select_charmap_by_encoding(face: &freetype::face::Face, encoding: u32) -> bool {
    for index in 0..face.num_charmaps() {
        let charmap = face.get_charmap(index as isize);
        if charmap.encoding() == encoding && face.set_charmap(&charmap).is_ok() {
            return true;
        }
    }

    false
}

fn font_name(face: &freetype::face::Face) -> String {
    let family = face.family_name().unwrap_or_else(|| "Font".to_string());
    match face.style_name().as_deref() {
        Some(style) if !style.eq_ignore_ascii_case("regular") && !style.trim().is_empty() => {
            format!("{family} {style}")
        }
        _ => family.to_string(),
    }
}

fn font_contains_text(face: &freetype::face::Face, text: &str) -> bool {
    text.chars()
        .filter(|ch| !ch.is_whitespace())
        .all(|ch| font_has_char(face, ch))
}

fn font_has_char(face: &freetype::face::Face, ch: char) -> bool {
    face.get_char_index(ch as usize)
        .is_some_and(|index| index != 0)
}

fn render_font_sample(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    preview: &FontPreviewFace,
) {
    let foreground = area.color();

    let width = f64::from(width.max(1));
    let height = f64::from(height.max(1));
    let available_width = (width - FONT_PREVIEW_MARGIN * 2.0).max(1.0);
    let mut y = FONT_PREVIEW_MARGIN;

    context.set_source_rgba(
        f64::from(foreground.red()),
        f64::from(foreground.green()),
        f64::from(foreground.blue()),
        f64::from(foreground.alpha()),
    );

    context.set_font_face(&preview.font_face);
    if font_contains_text(&preview.face, &preview.font_name) {
        draw_font_line(
            context,
            &preview.font_name,
            48.0,
            available_width,
            height,
            &mut y,
        );
    }

    y += FONT_SECTION_SPACING / 2.0;

    for text in [
        preview.lowercase_text,
        preview.uppercase_text,
        preview.punctuation_text,
    ]
    .into_iter()
    .flatten()
    {
        draw_font_line(context, text, 24.0, available_width, height, &mut y);
        if y > height {
            return;
        }
    }

    if let Some(sample_string) = preview.sample_string {
        y += FONT_SECTION_SPACING;
        for size in FONT_SAMPLE_SIZES {
            draw_font_line(
                context,
                sample_string,
                size,
                available_width,
                height,
                &mut y,
            );
            if y > height {
                break;
            }
        }
    }
}

fn draw_font_line(
    context: &cairo::Context,
    text: &str,
    size: f64,
    available_width: f64,
    available_height: f64,
    y: &mut f64,
) {
    context.set_font_size(size);

    let Ok(font_extents) = context.font_extents() else {
        return;
    };
    let Ok(text_extents) = context.text_extents(text) else {
        return;
    };

    if text_extents.x_advance() > available_width {
        let scale = (available_width / text_extents.x_advance()).clamp(0.2, 1.0);
        context.set_font_size(size * scale);
    }

    *y += font_extents.ascent()
        + font_extents.descent()
        + text_extents.y_advance()
        + FONT_LINE_SPACING / 2.0;
    if *y > available_height {
        return;
    }

    context.move_to(FONT_PREVIEW_MARGIN, *y);
    let _ = context.show_text(text);
    *y += FONT_LINE_SPACING / 2.0;
}

fn clear_pane(pane: &BinaryPreviewPane) {
    pane.pdf_preview.clear();
    clear_non_pdf_pane(pane);
}

fn clear_non_pdf_pane(pane: &BinaryPreviewPane) {
    pane.picture.set_paintable(Option::<&gdk::Paintable>::None);
    pane.picture.set_visible(false);
    pane.web_view.load_html("", Some("about:blank"));
    pane.web_view.set_visible(false);
    pane.font_area.set_visible(false);
    pane.font_preview.borrow_mut().take();
    pane.empty.set_visible(false);
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64_encode(bytes))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);

        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }

        if chunk.len() > 2 {
            output.push(TABLE[(third & 0b0011_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }

    output
}
