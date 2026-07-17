use gtk::gio::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::path::PathBuf;
use vte4::prelude::*;

use super::context_menu;

const TERMINAL_FILE_DROP_MIME_TYPES: &[&str] = &["text/uri-list"];
const MAX_TERMINAL_URI_LIST_BYTES: usize = 1024 * 1024;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalActivation {
    Url(String),
    File(TerminalFileActivation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalFileActivation {
    pub target: String,
    pub launch_dir: String,
}

pub fn configured_terminal(font_size: f64, columns: i64, rows: i64) -> vte4::Terminal {
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
    install_file_drop(&terminal);
    context_menu::install_terminal_context_menu(&terminal);
    terminal
}

pub fn set_font(terminal: &vte4::Terminal, font_size: f64) {
    terminal.set_font(Some(&pango::FontDescription::from_string(&format!(
        "monospace {}",
        font_size.round() as i32
    ))));
}

pub fn modified_enter_sequence(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<String> {
    if !matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
        return None;
    }

    let mut mask = 1;
    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        mask += 1;
    }
    if modifiers.contains(gdk::ModifierType::ALT_MASK) {
        mask += 2;
    }
    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        mask += 4;
    }
    if modifiers.contains(gdk::ModifierType::SUPER_MASK) {
        mask += 8;
    }

    (mask != 1).then(|| format!("\x1b[13;{mask}:1u"))
}

pub fn install_activation<F>(terminal: &vte4::Terminal, launch_dir: String, activate: F)
where
    F: Fn(TerminalActivation) + 'static,
{
    let click = gtk::GestureClick::builder().button(1).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let terminal = terminal.clone();

        move |gesture, press_count, x, y| {
            let modifiers = gesture.current_event_state();
            if press_count != 1
                || !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                || modifiers.contains(gdk::ModifierType::ALT_MASK)
            {
                return;
            }

            let Some(activation) = activation_at(&terminal, x, y, &launch_dir) else {
                return;
            };

            match &activation {
                TerminalActivation::Url(url) => {
                    log::info!("terminal url activation requested url={url}");
                }
                TerminalActivation::File(file) => {
                    log::info!(
                        "terminal file activation requested target={} launch_dir={}",
                        file.target,
                        file.launch_dir
                    );
                }
            }
            activate(activation);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    terminal.add_controller(click);
}

fn activation_at(
    terminal: &vte4::Terminal,
    x: f64,
    y: f64,
    launch_dir: &str,
) -> Option<TerminalActivation> {
    if let Some(hyperlink) = terminal
        .check_hyperlink_at(x, y)
        .and_then(|value| clean_activation_text(value.as_str()))
    {
        return Some(classify_activation(hyperlink, launch_dir));
    }

    let (matched, _) = terminal.check_match_at(x, y);
    matched
        .and_then(|value| clean_activation_text(value.as_str()))
        .map(|target| classify_activation(target, launch_dir))
}

fn classify_activation(target: String, launch_dir: &str) -> TerminalActivation {
    if is_http_url(&target) {
        TerminalActivation::Url(target)
    } else {
        TerminalActivation::File(TerminalFileActivation {
            target,
            launch_dir: launch_dir.to_string(),
        })
    }
}

fn is_http_url(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn clean_activation_text(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.trim_end_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ')' | ']' | '}'));
    (!value.is_empty()).then(|| value.to_string())
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

fn install_file_drop(terminal: &vte4::Terminal) {
    let file_target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    file_target.connect_drop({
        let terminal = terminal.clone();

        move |_, value, _, _| {
            let Some(paths) = file_list_value_paths(value) else {
                return false;
            };
            paste_dropped_paths(&terminal, paths)
        }
    });
    terminal.add_controller(file_target);

    let uri_target = gtk::DropTargetAsync::new(
        Some(gdk::ContentFormats::new(TERMINAL_FILE_DROP_MIME_TYPES)),
        gdk::DragAction::COPY,
    );
    uri_target.connect_drag_enter(|_, drop, _, _| uri_drop_action(drop));
    uri_target.connect_drag_motion(|_, drop, _, _| uri_drop_action(drop));
    uri_target.connect_drop({
        let terminal = terminal.clone();

        move |_, drop, _, _| {
            if uri_drop_action(drop).is_empty() {
                return false;
            }
            read_uri_list_drop(&terminal, drop);
            true
        }
    });
    terminal.add_controller(uri_target);
}

fn file_list_value_paths(value: &glib::Value) -> Option<Vec<PathBuf>> {
    let file_list = value.get::<gdk::FileList>().ok()?;
    let paths = file_list
        .files()
        .into_iter()
        .filter_map(|file| file.path())
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some(paths)
}

fn uri_drop_action(drop: &gdk::Drop) -> gdk::DragAction {
    let formats = drop.formats();
    if formats.contains_type(gdk::FileList::static_type())
        || !formats.contain_mime_type(TERMINAL_FILE_DROP_MIME_TYPES[0])
    {
        gdk::DragAction::empty()
    } else {
        gdk::DragAction::COPY
    }
}

fn read_uri_list_drop(terminal: &vte4::Terminal, drop: &gdk::Drop) {
    let terminal = terminal.clone();
    let drop = drop.clone();
    let finish_drop = drop.clone();
    drop.read_async(
        TERMINAL_FILE_DROP_MIME_TYPES,
        glib::Priority::default(),
        None::<&gio::Cancellable>,
        move |result| {
            let (stream, _) = match result {
                Ok(result) => result,
                Err(err) => {
                    log::warn!("terminal file drop uri-list read failed: {err}");
                    finish_drop.finish(gdk::DragAction::empty());
                    return;
                }
            };

            let finish_drop = finish_drop.clone();
            stream.read_all_async(
                vec![0; MAX_TERMINAL_URI_LIST_BYTES],
                glib::Priority::default(),
                None::<&gio::Cancellable>,
                move |result| {
                    let (buffer, bytes_read, partial_error) = match result {
                        Ok(result) => result,
                        Err((_, err)) => {
                            log::warn!("terminal file drop uri-list stream read failed: {err}");
                            finish_drop.finish(gdk::DragAction::empty());
                            return;
                        }
                    };
                    if let Some(err) = partial_error {
                        log::warn!("terminal file drop uri-list stream read partial: {err}");
                    }

                    let text = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let paths = uri_list_paths(&text);
                    if paste_dropped_paths(&terminal, paths) {
                        finish_drop.finish(gdk::DragAction::COPY);
                    } else {
                        log::warn!("terminal file drop uri-list contained no local paths");
                        finish_drop.finish(gdk::DragAction::empty());
                    }
                },
            );
        },
    );
}

fn uri_list_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|uri| gio::File::for_uri(uri).path())
        .collect()
}

fn paste_dropped_paths(terminal: &vte4::Terminal, paths: Vec<PathBuf>) -> bool {
    if paths.is_empty() {
        return false;
    }
    let text = paths
        .iter()
        .map(|path| shell_quote(&path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    terminal.paste_text(&text);
    terminal.grab_focus();
    log::info!("terminal file drop pasted paths count={}", paths.len());
    true
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
