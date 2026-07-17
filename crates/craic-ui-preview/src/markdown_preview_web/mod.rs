use crate::language_support::SyntaxHighlighter;
use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd, html,
};
use regex::Regex;
use std::ops::Range;
use std::sync::OnceLock;

const GITHUB_MARKDOWN_CSS: &str = include_str!("github-markdown.css");

const APP_INTEGRATION_CSS: &str = include_str!("app-integration.css");
pub const SOURCE_MAP_SCRIPT: &str = include_str!("markdown-preview.js");

pub fn markdown_document_html(markdown: &str) -> String {
    let mut document = String::new();
    document.push_str("<!doctype html>\n<html><head><meta charset=\"utf-8\">");
    document.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    document.push_str("<style>");
    document.push_str(GITHUB_MARKDOWN_CSS);
    document.push_str("</style><style>");
    document.push_str(APP_INTEGRATION_CSS);
    document.push_str("</style></head><body class=\"markdown-body\">");
    document.push_str(&markdown_fragment_html(markdown));
    document.push_str("</body></html>");
    document
}

pub fn markdown_fragment_html(markdown: &str) -> String {
    if markdown.is_empty() {
        return String::new();
    }

    let parser = Parser::new_ext(markdown, Options::all());
    let events: Vec<_> = parser.into_offset_iter().collect();
    let mut html_body = String::new();
    let mut segment_start = 0;
    let mut i = 0;

    while i < events.len() {
        if let Event::Start(Tag::CodeBlock(code_block_kind)) = &events[i].0 {
            push_html_with_source_anchors(&mut html_body, &events[segment_start..i]);
            html_body.push_str(&source_anchor(events[i].1.start));
            let language = match code_block_kind {
                CodeBlockKind::Fenced(info) => info
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                CodeBlockKind::Indented => String::new(),
            };

            i += 1;
            let mut code = String::new();
            while i < events.len() {
                match &events[i].0 {
                    Event::End(TagEnd::CodeBlock) => {
                        i += 1;
                        break;
                    }
                    Event::Text(text) | Event::Code(text) | Event::Html(text) => {
                        code.push_str(text);
                    }
                    Event::SoftBreak | Event::HardBreak => {
                        code.push('\n');
                    }
                    _ => {}
                }
                i += 1;
            }

            html_body.push_str(&render_code_block(language.as_str(), &code));
            segment_start = i;
            continue;
        }
        i += 1;
    }

    push_html_with_source_anchors(&mut html_body, &events[segment_start..]);
    html_body.push_str(&source_anchor(markdown.len()));
    html_body
}

fn push_html_with_source_anchors<'a>(html_body: &mut String, events: &[(Event<'a>, Range<usize>)]) {
    let mut anchored_events = Vec::with_capacity(events.len() * 2);
    let mut last_anchor = None;
    let mut link_or_image_depth = 0usize;
    let mut i = 0;

    while i < events.len() {
        let (event, range) = &events[i];
        if should_anchor_event(event) && last_anchor != Some(range.start) {
            anchored_events.push(Event::Html(CowStr::from(source_anchor(range.start))));
            last_anchor = Some(range.start);
        }

        if link_or_image_depth == 0 {
            if let Event::Start(Tag::Link { dest_url, .. }) = event {
                if is_video_url(dest_url) {
                    anchored_events.push(Event::Html(CowStr::from(video_html(dest_url))));
                    i = skip_link_events(events, i + 1);
                    continue;
                }
            }
        }

        match event {
            Event::Start(Tag::Link { .. }) | Event::Start(Tag::Image { .. }) => {
                link_or_image_depth += 1;
                push_alert_aware_event(&mut anchored_events, event);
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                push_alert_aware_event(&mut anchored_events, event);
                link_or_image_depth = link_or_image_depth.saturating_sub(1);
            }
            Event::Text(text) if link_or_image_depth == 0 => {
                push_text_with_auto_links(&mut anchored_events, text);
            }
            _ => push_alert_aware_event(&mut anchored_events, event),
        }
        i += 1;
    }

    html::push_html(html_body, anchored_events.into_iter());
}

fn skip_link_events<'a>(events: &[(Event<'a>, Range<usize>)], start: usize) -> usize {
    let mut depth = 1usize;

    for (index, (event, _)) in events.iter().enumerate().skip(start) {
        match event {
            Event::Start(Tag::Link { .. }) => depth += 1,
            Event::End(TagEnd::Link) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
    }

    events.len()
}

fn push_text_with_auto_links<'a>(events: &mut Vec<Event<'a>>, text: &CowStr<'a>) {
    let text = text.as_ref();
    let mut cursor = 0;
    let mut found_url = false;

    for matched in auto_link_url_regex().find_iter(text) {
        let start = matched.start();
        let end = matched.end();
        let trimmed_len = trimmed_auto_link_len(&text[start..end]);
        let url_end = start + trimmed_len;

        if url_end <= start {
            continue;
        }

        found_url = true;
        if cursor < start {
            events.push(Event::Text(CowStr::from(text[cursor..start].to_string())));
        }

        push_auto_link_url(events, &text[start..url_end]);
        cursor = url_end;
    }

    if !found_url {
        events.push(Event::Text(CowStr::from(text.to_string())));
    } else if cursor < text.len() {
        events.push(Event::Text(CowStr::from(text[cursor..].to_string())));
    }
}

fn push_auto_link_url<'a>(events: &mut Vec<Event<'a>>, url: &str) {
    if is_video_url(url) {
        events.push(Event::Html(CowStr::from(video_html(url))));
        return;
    }

    events.push(Event::Start(Tag::Link {
        link_type: LinkType::Autolink,
        dest_url: CowStr::from(url.to_string()),
        title: CowStr::from(String::new()),
        id: CowStr::from(String::new()),
    }));
    events.push(Event::Text(CowStr::from(url.to_string())));
    events.push(Event::End(TagEnd::Link));
}

fn auto_link_url_regex() -> &'static Regex {
    static URL_REGEX: OnceLock<Regex> = OnceLock::new();
    URL_REGEX.get_or_init(|| {
        Regex::new(r#"https?://[^\s<>"']+"#).expect("markdown auto-link regex should compile")
    })
}

fn trimmed_auto_link_len(url: &str) -> usize {
    let mut end = url.len();

    while end > 0 {
        let candidate = &url[..end];
        let Some(ch) = candidate.chars().last() else {
            break;
        };

        if matches!(ch, '.' | ',' | ';' | ':' | '!' | '?') {
            end -= ch.len_utf8();
            continue;
        }

        if matches!(ch, ')' | ']' | '}') && has_unbalanced_closing_delimiter(candidate, ch) {
            end -= ch.len_utf8();
            continue;
        }

        break;
    }

    end
}

fn has_unbalanced_closing_delimiter(text: &str, closing: char) -> bool {
    let opening = match closing {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => return false,
    };
    text.chars().filter(|ch| *ch == closing).count()
        > text.chars().filter(|ch| *ch == opening).count()
}

fn is_video_url(url: &str) -> bool {
    video_mime_type(url).is_some()
}

fn video_mime_type(url: &str) -> Option<&'static str> {
    let path_end = url
        .find('?')
        .unwrap_or(url.len())
        .min(url.find('#').unwrap_or(url.len()));
    let path = url[..path_end].trim_end_matches('/').to_ascii_lowercase();

    if path.ends_with(".mp4") || path.ends_with(".m4v") {
        Some("video/mp4")
    } else if path.ends_with(".webm") {
        Some("video/webm")
    } else if path.ends_with(".ogv") || path.ends_with(".ogg") {
        Some("video/ogg")
    } else if path.ends_with(".mov") {
        Some("video/quicktime")
    } else {
        None
    }
}

fn video_html(url: &str) -> String {
    let escaped_url = escape_html(url);
    let mime_type = video_mime_type(url).unwrap_or("video/mp4");
    format!(
        r#"<video class="craic-markdown-video" controls preload="metadata" playsinline><source src="{escaped_url}" type="{mime_type}"><a href="{escaped_url}">{escaped_url}</a></video>"#
    )
}

fn push_alert_aware_event<'a>(events: &mut Vec<Event<'a>>, event: &Event<'a>) {
    match event {
        Event::Start(Tag::BlockQuote(Some(kind))) => {
            events.push(Event::Html(CowStr::from(alert_open_html(*kind))));
            events.push(Event::Html(CowStr::from(alert_title_html(*kind))));
        }
        Event::End(TagEnd::BlockQuote(Some(_))) => {
            events.push(Event::Html(CowStr::from("</blockquote>\n")));
        }
        _ => events.push(event.clone()),
    }
}

fn alert_open_html(kind: BlockQuoteKind) -> String {
    format!(
        r#"<blockquote class="markdown-alert markdown-alert-{}">"#,
        alert_kind_class(kind)
    )
}

fn alert_title_html(kind: BlockQuoteKind) -> String {
    format!(
        r#"<p class="markdown-alert-title">{}{}</p>"#,
        alert_kind_icon(kind),
        alert_kind_title(kind)
    )
}

fn alert_kind_class(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "note",
        BlockQuoteKind::Tip => "tip",
        BlockQuoteKind::Important => "important",
        BlockQuoteKind::Warning => "warning",
        BlockQuoteKind::Caution => "caution",
    }
}

fn alert_kind_title(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "Note",
        BlockQuoteKind::Tip => "Tip",
        BlockQuoteKind::Important => "Important",
        BlockQuoteKind::Warning => "Warning",
        BlockQuoteKind::Caution => "Caution",
    }
}

fn alert_kind_icon(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => {
            r#"<svg class="markdown-alert-icon" viewBox="0 0 16 16" aria-hidden="true"><path d="m 8 0 c -4.410156 0 -8 3.589844 -8 8 s 3.589844 8 8 8 s 8 -3.589844 8 -8 s -3.589844 -8 -8 -8 z m 0 2 c 3.332031 0 6 2.667969 6 6 s -2.667969 6 -6 6 s -6 -2.667969 -6 -6 s 2.667969 -6 6 -6 z m 0 1.875 c -0.621094 0 -1.125 0.503906 -1.125 1.125 s 0.503906 1.125 1.125 1.125 s 1.125 -0.503906 1.125 -1.125 s -0.503906 -1.125 -1.125 -1.125 z m -1.523438 3.125 c -0.265624 0.011719 -0.476562 0.230469 -0.476562 0.5 c 0 0.277344 0.222656 0.5 0.5 0.5 h 0.5 v 3 h -0.5 c -0.277344 0 -0.5 0.222656 -0.5 0.5 s 0.222656 0.5 0.5 0.5 h 3 c 0.277344 0 0.5 -0.222656 0.5 -0.5 s -0.222656 -0.5 -0.5 -0.5 h -0.5 v -4 h -2.5 c -0.007812 0 -0.015625 0 -0.023438 0 z m 0 0"/></svg>"#
        }
        BlockQuoteKind::Tip => {
            r#"<svg class="markdown-alert-icon" viewBox="0 0 16 16" aria-hidden="true"><path d="m 7.996094 0 c -2.835938 0 -5.292969 2 -5.871094 4.777344 c -0.527344 2.535156 0.6875 5.035156 2.871094 6.324218 l 0.003906 0.898438 c 0 0.554688 0.449219 1 1 1 h 4 c 0.550781 0 1 -0.445312 1 -1 v -0.898438 c 2.183594 -1.292968 3.398438 -3.796874 2.871094 -6.332031 c -0.582032 -2.773437 -3.039063 -4.769531 -5.875 -4.769531 z m 0 2 c 1.898437 0 3.527344 1.320312 3.917968 3.179688 c 0.386719 1.863281 -0.574218 3.726562 -2.3125 4.484374 c -0.363281 0.160157 -0.597656 0.519532 -0.601562 0.914063 v 0.421875 h -2.003906 v -0.417969 c 0 -0.398437 -0.234375 -0.753906 -0.597656 -0.914062 c -1.742188 -0.761719 -2.703126 -2.625 -2.316407 -4.484375 s 2.011719 -3.183594 3.914063 -3.183594 z m 0 0"/><path d="m 6 15 c 0 0.554688 0.445312 1 1 1 h 2 c 0.554688 0 1 -0.445312 1 -1 v -1 h -4 z m 0 0"/><path d="m 6.644531 6.144531 c -0.195312 0.195313 -0.195312 0.515625 0 0.707031 l 1 1 c 0.195313 0.195313 0.511719 0.195313 0.707031 0 l 1 -1 c 0.195313 -0.191406 0.195313 -0.511718 0 -0.707031 c -0.195312 -0.191406 -0.511718 -0.191406 -0.707031 0 l -0.648437 0.648438 l -0.644532 -0.648438 c -0.195312 -0.191406 -0.511718 -0.191406 -0.707031 0 z m 0 0" fill-opacity="0.501961"/></svg>"#
        }
        BlockQuoteKind::Important => {
            r#"<svg class="markdown-alert-icon" viewBox="0 0 16 16" aria-hidden="true"><path d="m 8 0 c -4.410156 0 -8 3.589844 -8 8 s 3.589844 8 8 8 s 8 -3.589844 8 -8 s -3.589844 -8 -8 -8 z m 0 2 c 3.332031 0 6 2.667969 6 6 s -2.667969 6 -6 6 s -6 -2.667969 -6 -6 s 2.667969 -6 6 -6 z m 0 7.875 c -0.621094 0 -1.125 0.503906 -1.125 1.125 s 0.503906 1.125 1.125 1.125 s 1.125 -0.503906 1.125 -1.125 s -0.503906 -1.125 -1.125 -1.125 z m 0 0"/><path d="m 7 4 h 2 v 5 h -2 z m 0 0"/></svg>"#
        }
        BlockQuoteKind::Warning => {
            r#"<svg class="markdown-alert-icon" viewBox="0 0 16 16" aria-hidden="true"><path d="m 7.96875 5.957031 c 0.542969 -0.015625 1.046875 0.488281 1.03125 1.03125 v 1 c 0.007812 0.527344 -0.472656 1 -1 1 s -1.007812 -0.472656 -1 -1 v -1 c -0.007812 -0.464843 0.355469 -0.914062 0.8125 -1 c 0.050781 -0.015625 0.101562 -0.023437 0.15625 -0.03125 z m 0.03125 4.03125 c 0.550781 0 1 0.449219 1 1 s -0.449219 1 -1 1 s -1 -0.449219 -1 -1 s 0.449219 -1 1 -1 z m 0 0"/><path d="m 8 1.359375 c -0.769531 0 -1.535156 0.375 -1.941406 1.125 l -4.878906 9.0625 c -0.816407 1.515625 0.332031 3.441406 2.054687 3.441406 h 9.53125 c 1.722656 0 2.871094 -1.925781 2.054687 -3.441406 l -4.882812 -9.0625 c -0.402344 -0.75 -1.167969 -1.125 -1.9375 -1.125 z m -0.179688 2.070313 c 0.101563 -0.191407 0.257813 -0.191407 0.359376 0 l 4.878906 9.066406 c 0.144531 0.261718 0.003906 0.492187 -0.292969 0.492187 h -9.53125 c -0.296875 0 -0.4375 -0.230469 -0.296875 -0.492187 z m 0 0"/></svg>"#
        }
        BlockQuoteKind::Caution => {
            r#"<svg class="markdown-alert-icon" viewBox="0 0 16 16" aria-hidden="true"><path d="m 10.902344 0 h -5.800782 c -0.265624 0 -0.519531 0.105469 -0.707031 0.292969 l -4.101562 4.101562 c -0.1875 0.1875 -0.292969 0.441407 -0.292969 0.707031 v 5.796876 c 0 0.265624 0.105469 0.523437 0.292969 0.710937 l 4.101562 4.101563 c 0.1875 0.183593 0.441407 0.289062 0.707031 0.289062 h 5.796876 c 0.265624 0 0.523437 -0.105469 0.710937 -0.292969 l 4.097656 -4.101562 c 0.1875 -0.183594 0.292969 -0.441407 0.292969 -0.703125 v -5.800782 c 0 -0.265624 -0.105469 -0.519531 -0.292969 -0.707031 l -4.101562 -4.101562 c -0.1875 -0.1875 -0.441407 -0.292969 -0.703125 -0.292969 z m -0.417969 2 l 3.515625 3.515625 v 4.96875 l -3.515625 3.515625 h -4.96875 l -3.515625 -3.515625 v -4.96875 l 3.515625 -3.515625 z m 0 0"/><path d="m 6.996094 4 h 2 v 3 l -0.25 2 h -1.46875 l -0.28125 -2 z m 1 5.75 c 0.6875 0 1.25 0.558594 1.25 1.25 s -0.5625 1.25 -1.25 1.25 c -0.691406 0 -1.25 -0.558594 -1.25 -1.25 s 0.558594 -1.25 1.25 -1.25 z m 0 0"/></svg>"#
        }
    }
}

fn should_anchor_event(event: &Event<'_>) -> bool {
    match event {
        Event::Start(tag) => should_anchor_tag(tag),
        Event::Rule => true,
        _ => false,
    }
}

fn should_anchor_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Table(_)
            | Tag::MetadataBlock(_)
    )
}

fn source_anchor(offset: usize) -> String {
    format!(r#"<span class="source-anchor" data-source-start="{offset}"></span>"#)
}

fn render_code_block(language: &str, code: &str) -> String {
    let language = language.trim();
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.set_source(code);
    let mut ranges = highlighter.highlight_current();
    ranges.sort_by_key(|range| range.start);
    let code_len = code.len();

    let mut html = String::new();
    html.push_str("<pre><code");
    if !language.is_empty() {
        html.push_str(" class=\"language-");
        html.push_str(&sanitize_class(language));
        html.push('"');
    }
    html.push('>');

    let mut cursor = 0;
    for range in ranges {
        let mut start = range.start.min(code_len);
        let end = range.end.min(code_len);
        if !code.is_char_boundary(start) || !code.is_char_boundary(end) || start >= end {
            continue;
        }
        if end <= cursor {
            continue;
        }
        if start < cursor {
            start = cursor;
        }
        if cursor < start {
            html.push_str(&escape_html(&code[cursor..start]));
        }
        let (red, green, blue) = range.style.color();
        html.push_str("<span style=\"color:#");
        html.push_str(&format_color_hex(red, green, blue));
        html.push_str("\">");
        html.push_str(&escape_html(&code[start..end]));
        html.push_str("</span>");
        cursor = end;
    }
    if cursor < code_len {
        html.push_str(&escape_html(&code[cursor..]));
    }

    html.push_str("</code></pre>\n");
    html
}

fn format_color_hex(red: f64, green: f64, blue: f64) -> String {
    let scale_channel = |channel: f64| -> u8 {
        let value = (channel * 255.0).round();
        value.clamp(0.0, 255.0) as u8
    };
    format!(
        "{:02x}{:02x}{:02x}",
        scale_channel(red),
        scale_channel(green),
        scale_channel(blue)
    )
}

fn sanitize_class(language: &str) -> String {
    let mut class = String::with_capacity(language.len());
    for ch in language.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            class.push(ch.to_ascii_lowercase());
        } else {
            class.push('-');
        }
    }
    class
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
