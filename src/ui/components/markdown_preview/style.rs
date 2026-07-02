use gtk::gdk;
use std::sync::OnceLock;

const DEFAULT_CSS: &str = r#"
.craic-markdown-preview {
    background-color: @window_bg_color;
}
.craic-markdown-document {
    padding: 24px;
}
.craic-markdown-h1,
.craic-markdown-h2,
.craic-markdown-h3,
.craic-markdown-h4,
.craic-markdown-h5,
.craic-markdown-h6 {
    font-weight: bold;
}
.craic-markdown-h1 { font-size: 1.8em; }
.craic-markdown-h2 { font-size: 1.55em; }
.craic-markdown-h3 { font-size: 1.3em; }
.craic-markdown-h4 { font-size: 1.15em; }
.craic-markdown-h5,
.craic-markdown-h6 { font-size: 1.05em; }
.craic-markdown-code-inline,
.craic-markdown-pre {
    font-family: monospace;
}
.craic-markdown-code-inline {
    background-color: alpha(@view_fg_color, 0.08);
    border-radius: 5px;
    padding: 1px 4px;
}
.craic-markdown-pre {
    background-color: alpha(@view_fg_color, 0.08);
    border-radius: 8px;
    padding: 12px;
}
.craic-markdown-code-view,
.craic-markdown-code-view text,
.craic-markdown-code-scroll {
    background: transparent;
    background-color: transparent;
}
.craic-markdown-code-view text selection {
    background-color: alpha(@accent_color, 0.35);
    color: @accent_fg_color;
}
.craic-markdown-blockquote {
    border-left: 3px solid alpha(@view_fg_color, 0.25);
    padding-left: 12px;
}
.craic-markdown-alert-title-label {
    font-weight: bold;
}
.craic-markdown-alert-title-note,
.craic-markdown-alert-title-tip,
.craic-markdown-alert-title-important {
    color: @accent_color;
}
.craic-markdown-alert-title-warning {
    color: @warning_color;
}
.craic-markdown-alert-title-caution {
    color: @error_color;
}
.craic-markdown-table {
    border: 1px solid alpha(@view_fg_color, 0.12);
}
.craic-markdown-table-cell {
    border: 1px solid alpha(@view_fg_color, 0.12);
    padding: 6px 8px;
}
.craic-markdown-a {
    color: @accent_color;
    text-decoration-line: underline;
}
.craic-markdown-img-unresolved {
    color: alpha(@view_fg_color, 0.65);
}
"#;

static DEFAULT_CSS_INSTALLED: OnceLock<()> = OnceLock::new();

pub(super) fn install_default_css() {
    DEFAULT_CSS_INSTALLED.get_or_init(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(DEFAULT_CSS);

        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}
