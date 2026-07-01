use gtk::{gdk, pango};
use vte4::prelude::*;

use super::context_menu;

const TERMINAL_URL_MATCH_PATTERN: &str = r#"https?://[^\s<>"'`]+"#;
const TERMINAL_PATH_MATCH_PATTERN: &str = r#"(?x)
    (?:
        /[A-Za-z0-9._~+%/@=-]+
        |
        (?:\.{1,2}/)?(?:[A-Za-z0-9._~+%-]+/)+[A-Za-z0-9._~+%-]+
        |
        [A-Za-z0-9._~+%-]+\.[A-Za-z0-9._~+%-]+
    )
    (?::[0-9]+(?::[0-9]+)?)?
"#;

pub(crate) fn configured_terminal(font_size: f64, columns: i64, rows: i64) -> vte4::Terminal {
    let terminal = vte4::Terminal::new();
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);
    terminal.set_focusable(true);
    terminal.set_size(columns, rows);
    set_font(&terminal, font_size);
    terminal.set_scrollback_lines(10_000);
    terminal.set_scroll_on_keystroke(true);
    terminal.set_scroll_on_output(false);
    terminal.set_scroll_unit_is_pixels(true);
    terminal.set_enable_fallback_scrolling(false);
    terminal.set_mouse_autohide(true);
    terminal.set_bold_is_bright(true);
    terminal.set_enable_sixel(true);
    terminal.set_allow_hyperlink(true);
    terminal.set_enable_shaping(false);
    terminal.set_enable_bidi(false);
    terminal.set_colors(
        Some(&rgba(212, 212, 212)),
        Some(&rgba(30, 30, 30)),
        &ansi_palette().iter().collect::<Vec<_>>(),
    );
    terminal.set_color_cursor(Some(&rgba(174, 175, 173)));
    terminal.set_color_highlight(Some(&rgba(38, 79, 120)));
    terminal.set_color_highlight_foreground(Some(&rgba(255, 255, 255)));
    install_matches(&terminal);
    context_menu::install_terminal_context_menu(&terminal);
    terminal
}

pub(crate) fn set_font(terminal: &vte4::Terminal, font_size: f64) {
    terminal.set_font(Some(&pango::FontDescription::from_string(&format!(
        "monospace {}",
        font_size.round() as i32
    ))));
}

fn install_matches(terminal: &vte4::Terminal) {
    for (name, pattern) in [
        ("url", TERMINAL_URL_MATCH_PATTERN),
        ("path", TERMINAL_PATH_MATCH_PATTERN),
    ] {
        match vte4::Regex::for_match(pattern, 0) {
            Ok(regex) => {
                let tag = terminal.match_add_regex(&regex, 0);
                terminal.match_set_cursor_name(tag, "pointer");
                log::debug!("terminal match regex installed kind={name} tag={tag}");
            }
            Err(err) => log::warn!("terminal match regex failed kind={name}: {err}"),
        }
    }
}

fn ansi_palette() -> [gdk::RGBA; 16] {
    [
        rgba(0, 0, 0),
        rgba(205, 49, 49),
        rgba(13, 188, 121),
        rgba(229, 229, 16),
        rgba(36, 114, 200),
        rgba(188, 63, 188),
        rgba(17, 168, 205),
        rgba(229, 229, 229),
        rgba(102, 102, 102),
        rgba(241, 76, 76),
        rgba(35, 209, 139),
        rgba(245, 245, 67),
        rgba(59, 142, 234),
        rgba(214, 112, 214),
        rgba(41, 184, 219),
        rgba(255, 255, 255),
    ]
}

fn rgba(red: u8, green: u8, blue: u8) -> gdk::RGBA {
    gdk::RGBA::new(
        red as f32 / 255.0,
        green as f32 / 255.0,
        blue as f32 / 255.0,
        1.0,
    )
}
