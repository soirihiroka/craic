use adw::prelude::*;
use gtk::gdk::RGBA;

#[derive(Clone, Copy)]
pub(super) struct Color {
    pub(super) red: f64,
    pub(super) green: f64,
    pub(super) blue: f64,
    pub(super) alpha: f64,
}

impl Color {
    pub(super) const fn rgb(red: f64, green: f64, blue: f64) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    const fn rgb8(red: u8, green: u8, blue: u8) -> Self {
        Self::rgb(
            red as f64 / 255.0,
            green as f64 / 255.0,
            blue as f64 / 255.0,
        )
    }

    fn from_rgba(rgba: RGBA) -> Self {
        Self {
            red: rgba.red() as f64,
            green: rgba.green() as f64,
            blue: rgba.blue() as f64,
            alpha: rgba.alpha() as f64,
        }
    }

    pub(super) fn with_alpha(self, alpha: f64) -> Self {
        Self {
            alpha: alpha.clamp(0.0, 1.0),
            ..self
        }
    }
}

impl From<(f64, f64, f64)> for Color {
    fn from(value: (f64, f64, f64)) -> Self {
        Self {
            red: value.0,
            green: value.1,
            blue: value.2,
            alpha: 1.0,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct EditorTheme {
    pub(super) background: Color,
    pub(super) foreground: Color,
    pub(super) gutter_background: Color,
    pub(super) line_number: Color,
    pub(super) line_number_emphasis: Color,
    pub(super) folded_text: Color,
    pub(super) selection: Color,
    pub(super) search_match: Color,
    pub(super) search_match_current: Color,
    pub(super) cursor: Color,
    pub(super) fold_control_background: Color,
    pub(super) added_gutter_background: Color,
    pub(super) deleted_gutter_background: Color,
    pub(super) deleted_hint: Color,
    pub(super) spellcheck_underline: Color,
    pub(super) syntax_error_underline: Color,
}

pub(super) fn editor_theme(area: &gtk::DrawingArea) -> EditorTheme {
    let style_manager = adw::StyleManager::for_display(&area.display());
    let dark = style_manager.is_dark();
    let background = if dark {
        Color::rgb8(0x1d, 0x1d, 0x20)
    } else {
        Color::rgb8(0xff, 0xff, 0xff)
    };
    let foreground = Color::from_rgba(area.color());
    let gutter_background = if dark {
        Color::rgb8(0x22, 0x22, 0x26)
    } else {
        Color::rgb8(0xfa, 0xfa, 0xfb)
    };
    let accent = Color::from_rgba(style_manager.accent_color_rgba());
    let success = if dark {
        Color::rgb8(0x26, 0xa2, 0x69)
    } else {
        Color::rgb8(0x2e, 0xc2, 0x7e)
    };
    let error = if dark {
        Color::rgb8(0xc0, 0x1c, 0x28)
    } else {
        Color::rgb8(0xe0, 0x1b, 0x24)
    };

    EditorTheme {
        background,
        foreground,
        gutter_background,
        line_number: foreground.with_alpha(0.55),
        line_number_emphasis: foreground.with_alpha(0.86),
        folded_text: foreground.with_alpha(0.62),
        selection: accent.with_alpha(0.45),
        search_match: Color::rgb8(0xf6, 0xd3, 0x2d).with_alpha(if dark { 0.28 } else { 0.36 }),
        search_match_current: Color::rgb8(0xff, 0xbe, 0x6f).with_alpha(if dark {
            0.52
        } else {
            0.62
        }),
        cursor: accent,
        fold_control_background: foreground,
        added_gutter_background: success.with_alpha(0.26),
        deleted_gutter_background: error.with_alpha(0.28),
        deleted_hint: error.with_alpha(0.90),
        spellcheck_underline: Color::rgb8(0xf6, 0xd3, 0x2d).with_alpha(0.95),
        syntax_error_underline: error.with_alpha(0.95),
    }
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + (end - start) * amount.clamp(0.0, 1.0)
}

pub(super) fn lerp_color(start: Color, end: Color, amount: f64) -> Color {
    Color {
        red: lerp(start.red, end.red, amount),
        green: lerp(start.green, end.green, amount),
        blue: lerp(start.blue, end.blue, amount),
        alpha: lerp(start.alpha, end.alpha, amount),
    }
}
