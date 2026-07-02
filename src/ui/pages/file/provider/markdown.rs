use super::{PreviewMatchRequest, PreviewRequest};
use crate::git;
use crate::language_support::SyntaxHighlighter;
use crate::ui::components::markdown_preview::MarkdownPreviewDocument;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;

struct MarkdownPreviewLoad {
    text: String,
    document: MarkdownPreviewDocument,
    comparison: Option<git::FileComparison>,
    markdown_lint_issues: Vec<crate::markdown_lint::MarkdownLintIssue>,
    spellcheck_issues: Vec<crate::spellcheck::SpellcheckIssue>,
}

pub(in crate::ui::pages::file) fn show(request: PreviewRequest<'_>) {
    show_markdown(request, None);
}

pub(in crate::ui::pages::file) fn show_match(request: PreviewMatchRequest<'_>) {
    let selection = Some((request.start, request.end));
    show_markdown(request.into_preview_request(), selection);
}

fn show_markdown(request: PreviewRequest<'_>, selection: Option<(usize, usize)>) {
    request
        .right
        .show_editor_loading(request.file_path, "Markdown");

    let files = request.files.clone();
    let file_path = request.file_path.to_string();
    let apply_node_path = request.node_path.clone();
    let git = (request.ctx.system_ref().provider_kind == crate::system::ProviderKind::Local)
        .then(|| request.ctx.git())
        .flatten();
    let prefetched_bytes = request.prefetched_bytes.map(|bytes| bytes.to_vec());
    let apply_file_path = file_path.clone();
    let local_path = request.local_path.map(Path::to_path_buf);
    let disk_signature = super::disk_signature(request.info);
    let writable = request.info.capabilities.writable;
    let language = crate::ui::content::code_editor::language_hint_from_path(&file_path);

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || {
            super::super::repository_text_from_prefetch(prefetched_bytes, &file_path).map(|text| {
                let comparison = git.as_ref().and_then(|git| git.comparison(&file_path).ok());
                let allowlist = crate::spellcheck::manifest_allowlist_from_texts(&[(
                    &file_path,
                    text.as_str(),
                )]);
                let spellcheck_issues = crate::spellcheck::check_document(
                    &language,
                    Some(&file_path),
                    &text,
                    &allowlist,
                );
                let ignored_rules =
                    crate::workspace_config::markdown_lint_ignored_rules_from_file_access(
                        files.as_ref(),
                    );
                let markdown_lint_issues =
                    crate::markdown_lint::check_document(Some(&file_path), &text, &ignored_rules);
                let document = MarkdownPreviewDocument::parse(&text);
                MarkdownPreviewLoad {
                    text,
                    document,
                    comparison,
                    markdown_lint_issues,
                    spellcheck_issues,
                }
            })
        },
        move |right, result| match result {
            Ok(load) => {
                right.show_editor(
                    &apply_node_path,
                    &apply_file_path,
                    &load.text,
                    disk_signature,
                    writable,
                    load.comparison.as_ref(),
                    load.markdown_lint_issues,
                    load.spellcheck_issues,
                );
                if load.text.trim().is_empty() {
                    right
                        .file_view_split
                        .set_end_child(Some(&right.file_markdown_status));
                } else {
                    right
                        .file_markdown_preview
                        .set_document_with_base_path(load.document, local_path.as_deref());
                    let _ = right
                        .file_markdown_preview
                        .scroll_to_source_offset(right.file_editor.source_offset_at_scroll_top());
                    right
                        .file_view_split
                        .set_end_child(Some(&right.file_markdown_preview.root));
                }
                if let Some((start, end)) = selection {
                    right.file_editor.select_range(start, end);
                }
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

fn push_html_with_source_anchors<'a>(html_body: &mut String, events: &[(Event<'a>, Range<usize>)]) {
    let mut anchored_events = Vec::with_capacity(events.len() * 2);
    let mut last_anchor = None;

    for (event, range) in events {
        if should_anchor_event(event) && last_anchor != Some(range.start) {
            anchored_events.push(Event::Html(CowStr::from(source_anchor(range.start))));
            last_anchor = Some(range.start);
        }

        anchored_events.push(event.clone());
    }

    html::push_html(html_body, anchored_events.into_iter());
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

pub(super) fn markdown_fragment_to_html(markdown: &str) -> String {
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
