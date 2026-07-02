use adw::prelude::*;
use base64::Engine;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::{gdk, gdk_pixbuf, gio, pango};
use moka::sync::Cache;
use pulldown_cmark::Alignment;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::blocks::{
    MarkdownPreviewBlock, MarkdownPreviewBlockKind, RenderedImageItem, RenderedListItem,
    RenderedText, pango_escape,
};
use super::source_map::RenderedSourceAnchor;

const MAX_IMAGE_WIDTH: i32 = 1200;
const DEFAULT_BLOCK_IMAGE_HEIGHT: i32 = 240;
const IMAGE_CACHE_TTL_SECS: u64 = 30 * 60;

type ImageLoadResult = Result<Vec<u8>, String>;

#[derive(Clone)]
struct BlockImageState {
    pixbuf: Option<gdk_pixbuf::Pixbuf>,
    ratio: f32,
}

static IMAGE_CACHE: OnceLock<Cache<String, Vec<u8>>> = OnceLock::new();
static IMAGE_IN_FLIGHT: OnceLock<Mutex<HashMap<String, Vec<mpsc::Sender<ImageLoadResult>>>>> =
    OnceLock::new();

pub(super) fn render_document(
    document: &gtk::Box,
    blocks: &[MarkdownPreviewBlock],
    base_path: Option<PathBuf>,
) -> Vec<RenderedSourceAnchor> {
    clear_box(document);
    let mut anchors = Vec::new();

    for block in blocks {
        let widget = render_block(block, base_path.as_deref());
        widget.set_size_request(0, -1);
        widget.set_hexpand(true);
        document.append(&widget);

        if let Some(source) = block.source.clone() {
            anchors.push(RenderedSourceAnchor { widget, source });
        }
    }

    log::debug!("markdown preview rendered widgets={}", blocks.len());
    anchors
}

fn render_block(block: &MarkdownPreviewBlock, base_path: Option<&Path>) -> gtk::Widget {
    match &block.kind {
        MarkdownPreviewBlockKind::Heading { level, text } => {
            let markup = format!(
                "<span weight=\"bold\" size=\"{}\">{}</span>",
                heading_size(*level),
                text.markup
            );
            let label = markup_label(&markup, base_path);
            label.add_css_class("craic-markdown-heading");
            label.upcast()
        }
        MarkdownPreviewBlockKind::Paragraph(text) => markup_label(&text.markup, base_path).upcast(),
        MarkdownPreviewBlockKind::CodeBlock { code, language } => {
            render_code_block(code, language).upcast()
        }
        MarkdownPreviewBlockKind::Blockquote(text) => render_blockquote(text, base_path).upcast(),
        MarkdownPreviewBlockKind::List(items) => render_list(items, base_path).upcast(),
        MarkdownPreviewBlockKind::ThematicBreak => {
            gtk::Separator::new(gtk::Orientation::Horizontal).upcast()
        }
        MarkdownPreviewBlockKind::Table {
            headers,
            rows,
            alignments,
        } => render_table(headers, rows, alignments, base_path),
        MarkdownPreviewBlockKind::ImageGroup(images) => render_image_group(images, base_path),
    }
}

fn markup_label(markup: &str, base_path: Option<&Path>) -> gtk::Label {
    let label = gtk::Label::builder()
        .selectable(true)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .natural_wrap_mode(gtk::NaturalWrapMode::Word)
        .xalign(0.0)
        .halign(gtk::Align::Fill)
        .hexpand(true)
        .build();
    label.set_markup(markup);
    label.connect_activate_link({
        let base_path = base_path.map(Path::to_path_buf);
        move |label, uri| {
            confirm_open_external_uri(label.upcast_ref(), uri, base_path.as_deref());
            gtk::glib::Propagation::Stop
        }
    });
    label
}

fn heading_size(level: u8) -> &'static str {
    match level {
        1 => "xx-large",
        2 => "x-large",
        3 => "large",
        _ => "medium",
    }
}

fn render_code_block(code: &str, language: &Option<String>) -> gtk::Box {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build();
    container.add_css_class("craic-markdown-pre");

    if let Some(language) = language {
        let badge = gtk::Label::builder()
            .label(language.to_ascii_uppercase())
            .xalign(0.0)
            .halign(gtk::Align::Start)
            .build();
        badge.add_css_class("dim-label");
        badge.add_css_class("monospace");
        container.append(&badge);
    }

    let buffer = source_buffer_for_code(code, language.as_deref());
    let view = sourceview5::View::with_buffer(&buffer);
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_focusable(true);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.set_show_line_numbers(false);
    view.set_show_line_marks(false);
    view.set_highlight_current_line(false);
    view.set_left_margin(0);
    view.set_right_margin(0);
    view.set_top_margin(0);
    view.set_bottom_margin(0);
    view.set_hexpand(true);
    view.set_halign(gtk::Align::Fill);
    view.add_css_class("craic-markdown-code-view");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(0)
        .propagate_natural_width(false)
        .propagate_natural_height(true)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .child(&view)
        .build();
    scroller.set_size_request(0, -1);
    scroller.add_css_class("craic-markdown-code-scroll");
    container.append(&scroller);
    container
}

fn source_buffer_for_code(code: &str, language: Option<&str>) -> sourceview5::Buffer {
    let language = language
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .and_then(source_language_alias);
    let manager = sourceview5::LanguageManager::default();
    let source_language = language
        .as_deref()
        .and_then(|language| manager.language(language));

    let buffer = if let Some(language) = source_language {
        let buffer = sourceview5::Buffer::with_language(&language);
        buffer.set_highlight_syntax(true);
        buffer
    } else {
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_highlight_syntax(false);
        buffer
    };

    buffer.set_text(code);
    buffer.set_style_scheme(source_style_scheme().as_ref());
    buffer
}

fn source_language_alias(language: &str) -> Option<String> {
    match language.to_ascii_lowercase().as_str() {
        "" => None,
        "js" | "jsx" | "ts" | "tsx" => Some("typescript".to_string()),
        "py" => Some("python".to_string()),
        "rb" => Some("ruby".to_string()),
        "bash" | "shell" | "zsh" => Some("sh".to_string()),
        "cpp" | "cxx" | "c++" | "hpp" | "hxx" => Some("cpp".to_string()),
        "cs" | "csharp" => Some("c-sharp".to_string()),
        "yml" => Some("yaml".to_string()),
        "md" => Some("markdown".to_string()),
        "rs" => Some("rust".to_string()),
        "kt" => Some("kotlin".to_string()),
        other => Some(other.to_string()),
    }
}

fn source_style_scheme() -> Option<sourceview5::StyleScheme> {
    let manager = sourceview5::StyleSchemeManager::default();
    let dark = adw::StyleManager::default().is_dark();
    let candidates: &[&str] = if dark {
        &[
            "Adwaita-dark",
            "Adwaita",
            "builder-dark",
            "solarized-dark",
            "oblivion",
            "classic",
        ]
    } else {
        &[
            "Adwaita",
            "Adwaita-light",
            "solarized-light",
            "classic",
            "tango",
        ]
    };

    candidates.iter().find_map(|scheme| manager.scheme(scheme))
}

fn render_blockquote(text: &RenderedText, base_path: Option<&Path>) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build();
    let separator = gtk::Separator::new(gtk::Orientation::Vertical);
    separator.add_css_class("craic-markdown-blockquote-rule");
    row.append(&separator);

    let label = markup_label(&text.markup, base_path);
    label.add_css_class("craic-markdown-blockquote");
    row.append(&label);
    row
}

fn render_list(items: &[RenderedListItem], base_path: Option<&Path>) -> gtk::Label {
    let max_marker = items
        .iter()
        .map(|item| display_marker(&item.marker).chars().count() + item.depth * 2)
        .max()
        .unwrap_or(1);
    let markup = items
        .iter()
        .map(|item| {
            let marker = display_marker(&item.marker);
            let prefix_width = marker.chars().count() + item.depth * 2;
            let pad = " ".repeat(max_marker.saturating_sub(prefix_width) + 1);
            format!(
                "{}{}{}{}",
                "  ".repeat(item.depth),
                pango_escape(&marker),
                pad,
                item.text.markup,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let label = markup_label(&format!("<span>{markup}</span>"), base_path);
    label.add_css_class("craic-markdown-list");
    label
}

fn display_marker(marker: &str) -> String {
    match marker {
        "[x]" | "[X]" => "☑".to_string(),
        "[ ]" => "☐".to_string(),
        _ => marker.to_string(),
    }
}

fn render_table(
    headers: &[RenderedText],
    rows: &[Vec<RenderedText>],
    alignments: &[Alignment],
    base_path: Option<&Path>,
) -> gtk::Widget {
    let markup = table_markup(headers, rows, alignments);
    let label = markup_label(&format!("<tt>{markup}</tt>"), base_path);
    label.set_wrap(false);
    label.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(false);
    label.add_css_class("craic-markdown-table");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(0)
        .propagate_natural_width(false)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .child(&label)
        .build();
    scroller.set_size_request(0, -1);
    scroller.add_css_class("craic-markdown-table-scroller");
    scroller.upcast()
}

fn table_markup(
    headers: &[RenderedText],
    rows: &[Vec<RenderedText>],
    alignments: &[Alignment],
) -> String {
    if headers.is_empty() {
        return String::new();
    }

    let column_count = headers.len();
    let mut widths = vec![0usize; column_count];
    for (index, cell) in headers.iter().enumerate() {
        widths[index] = widths[index].max(cell.plain_text.chars().count());
    }
    for row in rows {
        for (index, cell) in row.iter().enumerate().take(column_count) {
            widths[index] = widths[index].max(cell.plain_text.chars().count());
        }
    }

    let mut lines = Vec::new();
    lines.push(table_row_markup(headers, &widths, alignments, true));
    lines.push(format!(
        "<span alpha=\"45%\">{}</span>",
        "─".repeat(widths.iter().sum::<usize>() + (column_count.saturating_sub(1) * 2))
    ));
    for row in rows {
        lines.push(table_row_markup(row, &widths, alignments, false));
    }
    lines.join("\n")
}

fn table_row_markup(
    cells: &[RenderedText],
    widths: &[usize],
    alignments: &[Alignment],
    bold: bool,
) -> String {
    widths
        .iter()
        .enumerate()
        .map(|(column, width)| {
            let empty = RenderedText {
                markup: String::new(),
                plain_text: String::new(),
            };
            let cell = cells.get(column).unwrap_or(&empty);
            let body_len = cell.plain_text.chars().count();
            let pad = width.saturating_sub(body_len);
            let left = match alignments.get(column).unwrap_or(&Alignment::None) {
                Alignment::Right => pad,
                Alignment::Center => pad / 2,
                Alignment::None | Alignment::Left => 0,
            };
            let right = pad.saturating_sub(left);
            let body = if bold {
                format!("<b>{}</b>", cell.markup)
            } else {
                cell.markup.clone()
            };
            format!("{}{}{}", " ".repeat(left), body, " ".repeat(right))
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn render_image_group(images: &[RenderedImageItem], base_path: Option<&Path>) -> gtk::Widget {
    if images.len() == 1
        && images
            .first()
            .and_then(|image| image.link_destination.as_ref())
            .is_none()
    {
        return render_block_image(&images[0], base_path);
    }

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .hexpand(false)
        .halign(gtk::Align::Start)
        .build();
    row.add_css_class("craic-markdown-image-group");

    for image in images {
        row.append(&render_inline_image(image, base_path));
    }

    row.upcast()
}

fn render_inline_image(image: &RenderedImageItem, base_path: Option<&Path>) -> gtk::Widget {
    let widget = image_widget(image, base_path, Some(24), None);
    if let Some(link) = image
        .link_destination
        .as_deref()
        .filter(|link| !link.trim().is_empty())
    {
        let wrapper = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .build();
        wrapper.add_css_class("craic-markdown-image-link");
        wrapper.append(&widget);
        wrapper.set_tooltip_text(Some(if image.alt.is_empty() {
            link
        } else {
            &image.alt
        }));

        let click = gtk::GestureClick::builder().button(0).build();
        let link = link.to_string();
        let base_path = base_path.map(Path::to_path_buf);
        let wrapper_for_click = wrapper.clone();
        click.connect_released(move |_, _, _, _| {
            confirm_open_external_uri(wrapper_for_click.upcast_ref(), &link, base_path.as_deref());
        });
        wrapper.add_controller(click);
        return wrapper.upcast();
    }
    widget
}

fn render_block_image(image: &RenderedImageItem, base_path: Option<&Path>) -> gtk::Widget {
    let ratio = image_ratio(image).unwrap_or(16.0 / 9.0);
    let state = Rc::new(RefCell::new(BlockImageState {
        pixbuf: None,
        ratio,
    }));
    let area = gtk::DrawingArea::builder()
        .content_width(0)
        .content_height(DEFAULT_BLOCK_IMAGE_HEIGHT)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build();
    area.set_size_request(0, -1);
    area.add_css_class("craic-markdown-img");
    area.set_draw_func({
        let state = state.clone();
        move |_, cr, width, height| draw_block_image(cr, width, height, &state.borrow())
    });
    area.connect_resize({
        let state = state.clone();
        move |area, width, _| update_block_image_height(area, width, state.borrow().ratio)
    });

    let status = unresolved_image_label(&image_alt_text(image), "loading image");
    status.set_valign(gtk::Align::Center);
    status.set_halign(gtk::Align::Center);

    let overlay = gtk::Overlay::builder()
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .child(&area)
        .build();
    overlay.add_overlay(&status);
    overlay.set_size_request(0, -1);

    load_block_image(image, base_path, &area, &status, &state);

    let clamp = adw::Clamp::builder()
        .maximum_size(MAX_IMAGE_WIDTH)
        .tightening_threshold(MAX_IMAGE_WIDTH)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .child(&overlay)
        .build();
    clamp.set_size_request(0, -1);
    clamp.set_overflow(gtk::Overflow::Hidden);
    clamp.upcast()
}

fn draw_block_image(cr: &cairo::Context, width: i32, height: i32, state: &BlockImageState) {
    let Some(pixbuf) = state.pixbuf.as_ref() else {
        return;
    };
    let image_width = pixbuf.width().max(1) as f64;
    let image_height = pixbuf.height().max(1) as f64;
    let available_width = width.max(1) as f64;
    let available_height = height.max(1) as f64;
    let scale = (available_width / image_width)
        .min(available_height / image_height)
        .max(0.0);
    let draw_width = image_width * scale;
    let draw_height = image_height * scale;
    let x = (available_width - draw_width) / 2.0;
    let y = (available_height - draw_height) / 2.0;

    let _ = cr.save();
    cr.rectangle(0.0, 0.0, available_width, available_height);
    cr.clip();
    cr.translate(x, y);
    cr.scale(scale, scale);
    cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
    let _ = cr.paint();
    let _ = cr.restore();
}

fn update_block_image_height(area: &gtk::DrawingArea, width: i32, ratio: f32) {
    if width <= 0 || ratio <= 0.0 {
        return;
    }
    let content_width = width.min(MAX_IMAGE_WIDTH).max(1);
    let content_height = ((content_width as f32 / ratio).round() as i32).max(1);
    if area.content_height() != content_height {
        area.set_content_height(content_height);
    }
}

fn set_block_image_pixbuf(
    area: &gtk::DrawingArea,
    status: &gtk::Label,
    state: &Rc<RefCell<BlockImageState>>,
    pixbuf: gdk_pixbuf::Pixbuf,
) {
    {
        let mut state = state.borrow_mut();
        let width = pixbuf.width().max(1);
        let height = pixbuf.height().max(1);
        state.ratio = width as f32 / height as f32;
        state.pixbuf = Some(pixbuf);
    }
    status.set_visible(false);
    update_block_image_height(area, area.allocated_width(), state.borrow().ratio);
    area.queue_draw();
}

fn load_block_image(
    image: &RenderedImageItem,
    base_path: Option<&Path>,
    area: &gtk::DrawingArea,
    status: &gtk::Label,
    state: &Rc<RefCell<BlockImageState>>,
) {
    let alt = image_alt_text(image);
    let Some(source) = image
        .source
        .as_deref()
        .filter(|source| !source.trim().is_empty())
    else {
        status.set_label("[image: missing image source]");
        return;
    };

    if let Some(bytes) = image_bytes_from_data_uri(source) {
        if let Some(pixbuf) = pixbuf_from_bytes(&bytes) {
            set_block_image_pixbuf(area, status, state, pixbuf);
        } else {
            status.set_label(&format!("[image: failed to decode {alt}]"));
        }
        return;
    }

    if let Some(file) = local_image_file(source, base_path) {
        if let Some(path) = file.path() {
            match gdk_pixbuf::Pixbuf::from_file(path) {
                Ok(pixbuf) => set_block_image_pixbuf(area, status, state, pixbuf),
                Err(err) => {
                    status.set_label(&format!("[image: failed to load {alt}]"));
                    log::warn!("markdown preview local block image failed source={source}: {err}");
                }
            }
        } else {
            status.set_label(&format!("[image: failed to resolve {alt}]"));
        }
        return;
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        load_remote_block_image(source, &alt, area, status, state);
    } else {
        status.set_label(&format!("[image: {source}]"));
    }
}

fn load_remote_block_image(
    source: &str,
    alt: &str,
    area: &gtk::DrawingArea,
    status: &gtk::Label,
    state: &Rc<RefCell<BlockImageState>>,
) {
    if let Some(bytes) = image_cache().get(source) {
        if let Some(pixbuf) = pixbuf_from_bytes(&bytes) {
            set_block_image_pixbuf(area, status, state, pixbuf);
        } else {
            status.set_label(&format!("[image: failed to decode {alt}]"));
        }
        return;
    }

    let (sender, receiver) = mpsc::channel();
    if register_image_request(source.to_string(), sender) {
        let uri = source.to_string();
        log::debug!("markdown preview remote block image fetch start uri={uri}");
        std::thread::spawn(move || {
            let result = reqwest::blocking::Client::builder()
                .user_agent("craic-markdown-preview")
                .build()
                .and_then(|client| client.get(&uri).send())
                .and_then(|response| response.error_for_status())
                .and_then(|response| response.bytes())
                .map(|bytes| bytes.to_vec())
                .map_err(|err| err.to_string());
            if let Ok(bytes) = result.as_ref() {
                image_cache().insert(uri.clone(), bytes.clone());
                log::debug!(
                    "markdown preview remote block image fetch complete uri={} bytes={}",
                    uri,
                    bytes.len()
                );
            } else if let Err(err) = result.as_ref() {
                log::warn!("markdown preview remote block image fetch failed uri={uri}: {err}");
            }
            complete_image_request(uri, result);
        });
    }

    gtk::glib::timeout_add_local(Duration::from_millis(75), {
        let area = area.clone();
        let status = status.clone();
        let state = state.clone();
        let alt = alt.to_string();
        let source = source.to_string();
        move || match receiver.try_recv() {
            Ok(Ok(bytes)) => {
                if let Some(pixbuf) = pixbuf_from_bytes(&bytes) {
                    set_block_image_pixbuf(&area, &status, &state, pixbuf);
                } else {
                    status.set_label(&format!("[image: failed to decode {alt}]"));
                    log::warn!("markdown preview remote block image decode failed uri={source}");
                }
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                status.set_label(&format!("[image: failed to load {alt}]"));
                log::warn!("markdown preview remote block image failed uri={source}: {err}");
                gtk::glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                status.set_label(&format!("[image: failed to load {alt}]"));
                log::warn!("markdown preview remote block image channel disconnected uri={source}");
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn image_widget(
    image: &RenderedImageItem,
    base_path: Option<&Path>,
    preferred_height: Option<i32>,
    ratio_frame: Option<&gtk::AspectFrame>,
) -> gtk::Widget {
    let alt = image_alt_text(image);
    let Some(source) = image
        .source
        .as_deref()
        .filter(|source| !source.trim().is_empty())
    else {
        return unresolved_image_label(&alt, "missing image source").upcast();
    };

    let picture = gtk::Picture::builder()
        .alternative_text(&alt)
        .can_shrink(preferred_height.is_none())
        .content_fit(gtk::ContentFit::Contain)
        .hexpand(preferred_height.is_none())
        .halign(if preferred_height.is_some() {
            gtk::Align::Start
        } else {
            gtk::Align::Fill
        })
        .valign(gtk::Align::Center)
        .build();
    picture.add_css_class("craic-markdown-img");
    if let Some(height) = preferred_height {
        picture.set_size_request(-1, height);
    }

    if let Some(texture) = texture_from_data_uri(source) {
        apply_texture_to_picture(&picture, &texture, image, preferred_height, ratio_frame);
        return picture.upcast();
    }

    if let Some(file) = local_image_file(source, base_path) {
        if let Ok(texture) = gdk::Texture::from_file(&file) {
            apply_texture_to_picture(&picture, &texture, image, preferred_height, ratio_frame);
        } else {
            apply_known_image_size(&picture, image, preferred_height, ratio_frame);
            picture.set_file(Some(&file));
        }
        return picture.upcast();
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        return remote_image_widget(picture, image, preferred_height, ratio_frame);
    }

    unresolved_image_label(&alt, source).upcast()
}

fn remote_image_widget(
    picture: gtk::Picture,
    image: &RenderedImageItem,
    preferred_height: Option<i32>,
    ratio_frame: Option<&gtk::AspectFrame>,
) -> gtk::Widget {
    let alt = image_alt_text(image);
    let Some(source) = image.source.as_deref() else {
        return unresolved_image_label(&alt, "missing image source").upcast();
    };
    let image = image.clone();
    let ratio_frame = ratio_frame.cloned();

    let stack = gtk::Stack::builder()
        .hhomogeneous(false)
        .vhomogeneous(false)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build();
    stack.set_size_request(0, -1);
    let status = unresolved_image_label(&alt, source);
    stack.add_named(&status, Some("status"));
    stack.add_named(&picture, Some("image"));
    stack.set_visible_child_name("status");

    if let Some(bytes) = image_cache().get(source) {
        log::debug!("markdown preview remote image cache hit uri={source}");
        apply_image_bytes(
            &picture,
            &stack,
            &status,
            &alt,
            source,
            &bytes,
            &image,
            preferred_height,
            ratio_frame.as_ref(),
        );
        return stack.upcast();
    }

    let (sender, receiver) = mpsc::channel();
    if register_image_request(source.to_string(), sender) {
        let uri = source.to_string();
        log::debug!("markdown preview remote image fetch start uri={uri}");
        std::thread::spawn(move || {
            let result = reqwest::blocking::Client::builder()
                .user_agent("craic-markdown-preview")
                .build()
                .and_then(|client| client.get(&uri).send())
                .and_then(|response| response.error_for_status())
                .and_then(|response| response.bytes())
                .map(|bytes| bytes.to_vec())
                .map_err(|err| err.to_string());
            if let Ok(bytes) = result.as_ref() {
                image_cache().insert(uri.clone(), bytes.clone());
                log::debug!(
                    "markdown preview remote image fetch complete uri={} bytes={}",
                    uri,
                    bytes.len()
                );
            } else if let Err(err) = result.as_ref() {
                log::warn!("markdown preview remote image fetch failed uri={uri}: {err}");
            }
            complete_image_request(uri, result);
        });
    }

    gtk::glib::timeout_add_local(Duration::from_millis(75), {
        let picture = picture.clone();
        let stack = stack.clone();
        let status = status.clone();
        let image = image.clone();
        let ratio_frame = ratio_frame.clone();
        let alt = alt.clone();
        let source = source.to_string();
        move || match receiver.try_recv() {
            Ok(Ok(bytes)) => {
                apply_image_bytes(
                    &picture,
                    &stack,
                    &status,
                    &alt,
                    &source,
                    &bytes,
                    &image,
                    preferred_height,
                    ratio_frame.as_ref(),
                );
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                status.set_label(&format!(
                    "[image: failed to load {}]",
                    if alt.trim().is_empty() { &source } else { &alt }
                ));
                log::warn!("markdown preview remote image failed uri={source}: {err}");
                gtk::glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                status.set_label(&format!(
                    "[image: failed to load {}]",
                    if alt.trim().is_empty() { &source } else { &alt }
                ));
                log::warn!("markdown preview remote image channel disconnected uri={source}");
                gtk::glib::ControlFlow::Break
            }
        }
    });

    stack.upcast()
}

fn apply_image_bytes(
    picture: &gtk::Picture,
    stack: &gtk::Stack,
    status: &gtk::Label,
    alt: &str,
    source: &str,
    bytes: &[u8],
    image: &RenderedImageItem,
    preferred_height: Option<i32>,
    ratio_frame: Option<&gtk::AspectFrame>,
) {
    if let Some(texture) = texture_from_bytes(bytes) {
        apply_texture_to_picture(picture, &texture, image, preferred_height, ratio_frame);
        stack.set_visible_child_name("image");
    } else {
        status.set_label(&format!(
            "[image: failed to decode {}]",
            if alt.trim().is_empty() { source } else { alt }
        ));
        log::warn!("markdown preview remote image decode failed uri={source}");
    }
}

fn apply_texture_to_picture(
    picture: &gtk::Picture,
    texture: &gdk::Texture,
    image: &RenderedImageItem,
    preferred_height: Option<i32>,
    ratio_frame: Option<&gtk::AspectFrame>,
) {
    picture.set_paintable(Some(texture));
    if image_dimensions(image).is_some() {
        apply_known_image_size(picture, image, preferred_height, ratio_frame);
    } else {
        apply_texture_size_to_picture(picture, texture, preferred_height, ratio_frame);
    }
}

fn apply_known_image_size(
    picture: &gtk::Picture,
    image: &RenderedImageItem,
    preferred_height: Option<i32>,
    ratio_frame: Option<&gtk::AspectFrame>,
) {
    if let Some(height) = preferred_height {
        let width = image_dimensions(image)
            .map(|(width, image_height)| ((height * width) / image_height).max(1))
            .unwrap_or(-1);
        picture.set_size_request(width, height);
    } else if let Some(frame) = ratio_frame {
        if let Some(ratio) = image_ratio(image) {
            frame.set_ratio(ratio);
            frame.queue_resize();
        }
        picture.set_size_request(0, -1);
    } else if let Some((width, height)) = image_dimensions(image) {
        let display_width = width.min(MAX_IMAGE_WIDTH).max(1);
        let display_height = ((display_width * height) / width).max(1);
        picture.set_size_request(0, display_height);
    }
}

fn apply_texture_size_to_picture(
    picture: &gtk::Picture,
    texture: &gdk::Texture,
    preferred_height: Option<i32>,
    ratio_frame: Option<&gtk::AspectFrame>,
) {
    let width = texture.width().max(1);
    let height = texture.height().max(1);
    if let Some(preferred_height) = preferred_height {
        let preferred_width = ((preferred_height * width) / height).max(1);
        picture.set_size_request(preferred_width, preferred_height);
        return;
    }
    if let Some(frame) = ratio_frame {
        frame.set_ratio(width as f32 / height as f32);
        frame.queue_resize();
        picture.set_size_request(0, -1);
        return;
    }

    let allocated_width = picture.allocated_width();
    let display_width = if allocated_width > 0 {
        allocated_width.min(width).min(MAX_IMAGE_WIDTH)
    } else {
        width.min(MAX_IMAGE_WIDTH)
    }
    .max(1);
    let display_height = ((display_width * height) / width).max(1);
    picture.set_size_request(0, display_height);
}

fn image_dimensions(image: &RenderedImageItem) -> Option<(i32, i32)> {
    Some((image.width?, image.height?)).filter(|(width, height)| *width > 0 && *height > 0)
}

fn image_ratio(image: &RenderedImageItem) -> Option<f32> {
    image_dimensions(image).map(|(width, height)| width as f32 / height as f32)
}

fn unresolved_image_label(alt: &str, detail: &str) -> gtk::Label {
    let label_text = if alt.trim().is_empty() { detail } else { alt };
    let label = gtk::Label::builder()
        .label(&format!("[image: {label_text}]"))
        .selectable(true)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .natural_wrap_mode(gtk::NaturalWrapMode::Word)
        .xalign(0.0)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build();
    label.add_css_class("craic-markdown-img-unresolved");
    label
}

fn image_alt_text(image: &RenderedImageItem) -> String {
    if !image.alt.trim().is_empty() {
        image.alt.clone()
    } else if let Some(title) = image
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    {
        title.to_string()
    } else {
        image.source.clone().unwrap_or_else(|| "Image".to_string())
    }
}

fn local_image_file(source: &str, base_path: Option<&Path>) -> Option<gio::File> {
    if source.starts_with("file://") {
        return Some(gio::File::for_uri(source));
    }
    if source.contains("://") || source.starts_with("data:") {
        return None;
    }

    let path = Path::new(source);
    if path.is_absolute() {
        Some(gio::File::for_path(path))
    } else {
        base_path.map(|base_path| {
            let base_dir = if base_path.is_dir() {
                base_path
            } else {
                base_path.parent().unwrap_or(base_path)
            };
            gio::File::for_path(base_dir.join(path))
        })
    }
}

fn texture_from_bytes(bytes: &[u8]) -> Option<gdk::Texture> {
    pixbuf_from_bytes(bytes).map(|pixbuf| gdk::Texture::for_pixbuf(&pixbuf))
}

fn pixbuf_from_bytes(bytes: &[u8]) -> Option<gdk_pixbuf::Pixbuf> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    loader.pixbuf()
}

fn texture_from_data_uri(source: &str) -> Option<gdk::Texture> {
    image_bytes_from_data_uri(source)
        .as_deref()
        .and_then(texture_from_bytes)
}

fn image_bytes_from_data_uri(source: &str) -> Option<Vec<u8>> {
    let marker = ";base64,";
    let marker_index = source.find(marker)?;
    if !source[..marker_index].starts_with("data:image/") {
        return None;
    }
    base64::engine::general_purpose::STANDARD
        .decode(&source[marker_index + marker.len()..])
        .ok()
}

fn image_cache() -> &'static Cache<String, Vec<u8>> {
    IMAGE_CACHE.get_or_init(|| {
        Cache::builder()
            .max_capacity(128)
            .time_to_live(Duration::from_secs(IMAGE_CACHE_TTL_SECS))
            .build()
    })
}

fn image_in_flight() -> &'static Mutex<HashMap<String, Vec<mpsc::Sender<ImageLoadResult>>>> {
    IMAGE_IN_FLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_image_request(uri: String, sender: mpsc::Sender<ImageLoadResult>) -> bool {
    let mut in_flight = image_in_flight()
        .lock()
        .expect("markdown preview image in-flight lock");
    if let Some(senders) = in_flight.get_mut(&uri) {
        senders.push(sender);
        false
    } else {
        in_flight.insert(uri, vec![sender]);
        true
    }
}

fn complete_image_request(uri: String, result: ImageLoadResult) {
    let senders = image_in_flight()
        .lock()
        .expect("markdown preview image in-flight lock")
        .remove(&uri)
        .unwrap_or_default();

    for sender in senders {
        let _ = sender.send(result.clone());
    }
}

fn confirm_open_external_uri(parent: &gtk::Widget, uri: &str, base_path: Option<&Path>) {
    let Some(uri) = resolved_external_uri(uri, base_path) else {
        log::debug!("markdown preview link ignored uri={uri}");
        return;
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Open Link?")
        .body(&uri)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("open", "Open");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let parent = parent
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());
    dialog.choose(
        parent.as_ref(),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() != "open" {
                log::debug!("markdown preview link open cancelled uri={uri}");
                return;
            }

            open_external_uri(&uri);
        },
    );
}

fn open_external_uri(uri: &str) {
    if let Err(err) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
        log::warn!("failed to open markdown preview URI externally: {err}");
    }
}

fn resolved_external_uri(uri: &str, base_path: Option<&Path>) -> Option<String> {
    let uri = uri.trim();
    if uri.is_empty() || uri.starts_with('#') {
        return None;
    }

    let scheme = uri
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .unwrap_or_default();
    if ["about", "data", "javascript"]
        .iter()
        .any(|blocked| scheme.eq_ignore_ascii_case(blocked))
    {
        return None;
    }
    if !scheme.is_empty() {
        if ["http", "https", "mailto", "file"]
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
        {
            return Some(uri.to_string());
        }
        log::warn!("markdown preview blocked unsupported URI scheme scheme={scheme}");
        return None;
    }

    base_path.map(|base_path| {
        let base_dir = if base_path.is_dir() {
            base_path
        } else {
            base_path.parent().unwrap_or(base_path)
        };
        gio::File::for_path(base_dir.join(uri)).uri().to_string()
    })
}

fn clear_box(box_: &gtk::Box) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child);
    }
}
