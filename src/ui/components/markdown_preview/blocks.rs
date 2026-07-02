use pulldown_cmark::Alignment;
use std::ops::Range;

use super::ast::{MarkdownElement, MarkdownNode, attr_value, text_content};

#[derive(Clone, Debug)]
pub(super) struct MarkdownPreviewBlock {
    pub(super) kind: MarkdownPreviewBlockKind,
    pub(super) source: Option<Range<usize>>,
}

#[derive(Clone, Debug)]
pub(super) enum MarkdownPreviewBlockKind {
    Heading {
        level: u8,
        text: RenderedText,
    },
    Paragraph {
        text: RenderedText,
        alignment: BlockAlignment,
    },
    CodeBlock {
        code: String,
        language: Option<String>,
    },
    Blockquote {
        text: RenderedText,
        alert: Option<MarkdownAlertKind>,
    },
    List(Vec<RenderedListItem>),
    ThematicBreak,
    Table {
        headers: Vec<RenderedText>,
        rows: Vec<Vec<RenderedText>>,
        alignments: Vec<Alignment>,
    },
    ImageGroup {
        images: Vec<RenderedImageItem>,
        alignment: BlockAlignment,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BlockAlignment {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MarkdownAlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Debug)]
pub(super) struct RenderedText {
    pub(super) markup: String,
    pub(super) plain_text: String,
}

#[derive(Clone, Debug)]
pub(super) struct RenderedListItem {
    pub(super) text: RenderedText,
    pub(super) depth: usize,
    pub(super) marker: String,
}

#[derive(Clone, Debug)]
pub(super) struct RenderedImageItem {
    pub(super) alt: String,
    pub(super) source: Option<String>,
    pub(super) title: Option<String>,
    pub(super) link_destination: Option<String>,
    pub(super) width: Option<i32>,
    pub(super) height: Option<i32>,
}

pub(super) fn blocks_from_nodes(nodes: &[MarkdownNode]) -> Vec<MarkdownPreviewBlock> {
    let mut blocks = Vec::new();
    append_blocks_from_nodes(&mut blocks, nodes, 0);
    log::debug!(
        "markdown preview semantic blocks built blocks={} images={} tables={} lists={}",
        blocks.len(),
        blocks
            .iter()
            .filter(|block| matches!(block.kind, MarkdownPreviewBlockKind::ImageGroup { .. }))
            .count(),
        blocks
            .iter()
            .filter(|block| matches!(block.kind, MarkdownPreviewBlockKind::Table { .. }))
            .count(),
        blocks
            .iter()
            .filter(|block| matches!(block.kind, MarkdownPreviewBlockKind::List(_)))
            .count(),
    );
    blocks
}

pub(super) fn collect_style_css(nodes: &[MarkdownNode]) -> String {
    let mut css = String::new();
    for node in nodes {
        let MarkdownNode::Element(element) = node else {
            continue;
        };

        if element.tag == "style" {
            css.push_str(&text_content(&element.children));
            css.push('\n');
        } else {
            css.push_str(&collect_style_css(&element.children));
        }
    }
    css
}

fn append_blocks_from_nodes(
    blocks: &mut Vec<MarkdownPreviewBlock>,
    nodes: &[MarkdownNode],
    list_depth: usize,
) {
    for node in nodes {
        match node {
            MarkdownNode::Text { text, source } => {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                blocks.push(MarkdownPreviewBlock {
                    kind: MarkdownPreviewBlockKind::Paragraph {
                        text: RenderedText::plain(text),
                        alignment: BlockAlignment::Start,
                    },
                    source: source.clone(),
                });
            }
            MarkdownNode::Element(element) => {
                append_block_from_element(blocks, element, list_depth)
            }
        }
    }
}

fn append_block_from_element(
    blocks: &mut Vec<MarkdownPreviewBlock>,
    element: &MarkdownElement,
    list_depth: usize,
) {
    match element.tag.as_str() {
        "p" | "dt" | "dd" => append_paragraph_block(blocks, element),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = element
                .tag
                .strip_prefix('h')
                .and_then(|level| level.parse::<u8>().ok())
                .unwrap_or(1);
            let text = inline_text(&element.children);
            if !text.plain_text.trim().is_empty() {
                blocks.push(MarkdownPreviewBlock {
                    kind: MarkdownPreviewBlockKind::Heading { level, text },
                    source: element.source.clone(),
                });
            }
        }
        "pre" => {
            let language = attr_value(&element.attrs, "data-language")
                .filter(|language| !language.trim().is_empty());
            blocks.push(MarkdownPreviewBlock {
                kind: MarkdownPreviewBlockKind::CodeBlock {
                    code: text_content(&element.children),
                    language,
                },
                source: element.source.clone(),
            });
        }
        "blockquote" => {
            let mut nested = Vec::new();
            append_blocks_from_nodes(&mut nested, &element.children, list_depth);
            let text = block_text(&nested);
            if !text.plain_text.trim().is_empty() {
                blocks.push(MarkdownPreviewBlock {
                    kind: MarkdownPreviewBlockKind::Blockquote {
                        text,
                        alert: markdown_alert_kind(element),
                    },
                    source: element.source.clone(),
                });
            }
        }
        "ul" | "ol" => {
            let items = list_items(element, list_depth);
            if !items.is_empty() {
                blocks.push(MarkdownPreviewBlock {
                    kind: MarkdownPreviewBlockKind::List(items),
                    source: element.source.clone(),
                });
            }
        }
        "table" => {
            if let Some((headers, rows, alignments)) = table_parts(element) {
                blocks.push(MarkdownPreviewBlock {
                    kind: MarkdownPreviewBlockKind::Table {
                        headers,
                        rows,
                        alignments,
                    },
                    source: element.source.clone(),
                });
            }
        }
        "img" => {
            blocks.push(MarkdownPreviewBlock {
                kind: MarkdownPreviewBlockKind::ImageGroup {
                    images: vec![image_item(element, None)],
                    alignment: BlockAlignment::Start,
                },
                source: element.source.clone(),
            });
        }
        "hr" => blocks.push(MarkdownPreviewBlock {
            kind: MarkdownPreviewBlockKind::ThematicBreak,
            source: element.source.clone(),
        }),
        "div" => append_container_blocks(blocks, element, list_depth),
        "script" | "style" => {}
        _ => append_blocks_from_nodes(blocks, &element.children, list_depth),
    }
}

fn append_container_blocks(
    blocks: &mut Vec<MarkdownPreviewBlock>,
    element: &MarkdownElement,
    list_depth: usize,
) {
    let alignment = block_alignment(element);
    if let Some(images) = standalone_image_group(&element.children) {
        blocks.push(MarkdownPreviewBlock {
            kind: MarkdownPreviewBlockKind::ImageGroup { images, alignment },
            source: element.source.clone(),
        });
        return;
    }

    let first_nested = blocks.len();
    append_blocks_from_nodes(blocks, &element.children, list_depth);
    if alignment == BlockAlignment::Start {
        return;
    }

    for block in &mut blocks[first_nested..] {
        match &mut block.kind {
            MarkdownPreviewBlockKind::Paragraph {
                alignment: block_alignment,
                ..
            }
            | MarkdownPreviewBlockKind::ImageGroup {
                alignment: block_alignment,
                ..
            } => *block_alignment = alignment,
            _ => {}
        }
    }
}

fn append_paragraph_block(blocks: &mut Vec<MarkdownPreviewBlock>, element: &MarkdownElement) {
    let alignment = block_alignment(element);
    if let Some(images) = standalone_image_group(&element.children) {
        blocks.push(MarkdownPreviewBlock {
            kind: MarkdownPreviewBlockKind::ImageGroup { images, alignment },
            source: element.source.clone(),
        });
        return;
    }

    let text = inline_text(&element.children);
    if text.plain_text.trim().is_empty() {
        return;
    }

    blocks.push(MarkdownPreviewBlock {
        kind: MarkdownPreviewBlockKind::Paragraph { text, alignment },
        source: element.source.clone(),
    });
}

fn block_alignment(element: &MarkdownElement) -> BlockAlignment {
    let align = attr_value(&element.attrs, "align")
        .or_else(|| {
            attr_value(&element.attrs, "style")
                .and_then(|style| css_text_align(&style).map(str::to_string))
        })
        .unwrap_or_default()
        .to_ascii_lowercase();

    match align.trim() {
        "center" | "middle" => BlockAlignment::Center,
        "right" | "end" => BlockAlignment::End,
        _ => BlockAlignment::Start,
    }
}

fn css_text_align(style: &str) -> Option<&str> {
    style.split(';').find_map(|part| {
        let (name, value) = part.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("text-align")
            .then(|| value.trim())
            .filter(|value| !value.is_empty())
    })
}

fn markdown_alert_kind(element: &MarkdownElement) -> Option<MarkdownAlertKind> {
    let class = attr_value(&element.attrs, "class")?;
    class.split_whitespace().find_map(|class| {
        let kind = class.strip_prefix("markdown-alert-")?;
        match kind {
            "note" => Some(MarkdownAlertKind::Note),
            "tip" => Some(MarkdownAlertKind::Tip),
            "important" => Some(MarkdownAlertKind::Important),
            "warning" => Some(MarkdownAlertKind::Warning),
            "caution" => Some(MarkdownAlertKind::Caution),
            _ => None,
        }
    })
}

fn list_items(element: &MarkdownElement, depth: usize) -> Vec<RenderedListItem> {
    let mut output = Vec::new();
    let ordered = element.tag == "ol";
    let mut index = attr_value(&element.attrs, "start")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1);

    for child in &element.children {
        let MarkdownNode::Element(item) = child else {
            continue;
        };
        if item.tag != "li" {
            continue;
        }

        let checkbox = first_checkbox(item);
        let mut inline_nodes = Vec::new();
        let mut nested_lists = Vec::new();

        for child in &item.children {
            match child {
                MarkdownNode::Element(paragraph)
                    if paragraph.tag == "p" && inline_nodes.is_empty() =>
                {
                    inline_nodes.extend(
                        paragraph
                            .children
                            .iter()
                            .filter(|node| !is_checkbox_node(node))
                            .cloned(),
                    );
                }
                MarkdownNode::Element(list) if list.tag == "ul" || list.tag == "ol" => {
                    nested_lists.extend(list_items(list, depth + 1));
                }
                _ if !is_checkbox_node(child) => inline_nodes.push(child.clone()),
                _ => {}
            }
        }

        let raw_text = inline_text(&inline_nodes);
        let text = RenderedText {
            markup: raw_text.markup.trim().to_string(),
            plain_text: raw_text.plain_text.trim().to_string(),
        };
        let marker = checkbox.unwrap_or_else(|| {
            if ordered {
                let marker = format!("{index}.");
                index += 1;
                marker
            } else {
                "•".to_string()
            }
        });

        if !text.plain_text.is_empty() {
            output.push(RenderedListItem {
                text,
                depth,
                marker,
            });
        }
        output.extend(nested_lists);
        if !ordered {
            index += 1;
        }
    }

    output
}

fn first_checkbox(element: &MarkdownElement) -> Option<String> {
    for child in &element.children {
        let MarkdownNode::Element(input) = child else {
            continue;
        };
        if input.tag == "input"
            && attr_value(&input.attrs, "type").is_some_and(|value| value == "checkbox")
        {
            return Some(
                if attr_value(&input.attrs, "checked").is_some_and(|value| value == "true") {
                    "[x]".to_string()
                } else {
                    "[ ]".to_string()
                },
            );
        }
    }
    None
}

fn is_checkbox_node(node: &MarkdownNode) -> bool {
    matches!(
        node,
        MarkdownNode::Element(element)
            if element.tag == "input"
                && attr_value(&element.attrs, "type").is_some_and(|value| value == "checkbox")
    )
}

fn table_parts(
    element: &MarkdownElement,
) -> Option<(Vec<RenderedText>, Vec<Vec<RenderedText>>, Vec<Alignment>)> {
    let alignments = attr_value(&element.attrs, "data-alignments")
        .as_deref()
        .map(parse_alignment_attr)
        .unwrap_or_default();

    let mut header_rows = Vec::new();
    let mut body_rows = Vec::new();
    collect_table_rows(element, false, &mut header_rows, &mut body_rows);
    let headers = if let Some(headers) = header_rows.into_iter().next() {
        headers
    } else if !body_rows.is_empty() {
        body_rows.remove(0)
    } else {
        return None;
    };

    Some((headers, body_rows, alignments))
}

fn collect_table_rows(
    element: &MarkdownElement,
    in_head: bool,
    header_rows: &mut Vec<Vec<RenderedText>>,
    body_rows: &mut Vec<Vec<RenderedText>>,
) {
    let in_head = in_head || element.tag == "thead";
    if element.tag == "tr" {
        let row = element
            .children
            .iter()
            .filter_map(|node| match node {
                MarkdownNode::Element(cell) if cell.tag == "td" || cell.tag == "th" => {
                    Some(inline_text(&cell.children))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if !row.is_empty() {
            if in_head {
                header_rows.push(row);
            } else {
                body_rows.push(row);
            }
        }
        return;
    }

    for child in &element.children {
        if let MarkdownNode::Element(child) = child {
            collect_table_rows(child, in_head, header_rows, body_rows);
        }
    }
}

fn parse_alignment_attr(input: &str) -> Vec<Alignment> {
    input
        .split(',')
        .map(|alignment| match alignment.trim() {
            "left" => Alignment::Left,
            "center" => Alignment::Center,
            "right" => Alignment::Right,
            _ => Alignment::None,
        })
        .collect()
}

fn standalone_image_group(nodes: &[MarkdownNode]) -> Option<Vec<RenderedImageItem>> {
    let mut images = Vec::new();
    for node in nodes {
        match node {
            MarkdownNode::Text { text, .. } if text.trim().is_empty() => {}
            MarkdownNode::Element(element) => {
                let item = rendered_image_item(element)?;
                images.push(item);
            }
            _ => return None,
        }
    }
    (!images.is_empty()).then_some(images)
}

fn rendered_image_item(element: &MarkdownElement) -> Option<RenderedImageItem> {
    match element.tag.as_str() {
        "img" => Some(image_item(element, None)),
        "a" => {
            let href = attr_value(&element.attrs, "href");
            let mut images = standalone_image_group(&element.children)?;
            (images.len() == 1).then(|| {
                let mut image = images.remove(0);
                image.link_destination = href;
                image
            })
        }
        _ => None,
    }
}

fn image_item(element: &MarkdownElement, link_destination: Option<String>) -> RenderedImageItem {
    RenderedImageItem {
        alt: attr_value(&element.attrs, "alt").unwrap_or_else(|| text_content(&element.children)),
        source: attr_value(&element.attrs, "src"),
        title: attr_value(&element.attrs, "title").filter(|title| !title.trim().is_empty()),
        link_destination,
        width: attr_value(&element.attrs, "width").and_then(|value| parse_image_dimension(&value)),
        height: attr_value(&element.attrs, "height")
            .and_then(|value| parse_image_dimension(&value)),
    }
}

fn parse_image_dimension(value: &str) -> Option<i32> {
    let value = value.trim();
    if value.contains('%') {
        return None;
    }

    let numeric = value
        .trim_end_matches("px")
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    numeric
        .parse::<f64>()
        .ok()
        .map(|value| value.round() as i32)
        .filter(|value| *value > 0)
}

fn inline_text(nodes: &[MarkdownNode]) -> RenderedText {
    inline_text_with_linkify(nodes, true)
}

fn inline_text_with_linkify(nodes: &[MarkdownNode], linkify: bool) -> RenderedText {
    let mut markup = String::new();
    let mut plain_text = String::new();
    append_inline_text(nodes, &mut markup, &mut plain_text, linkify);
    RenderedText { markup, plain_text }
}

fn append_inline_text(
    nodes: &[MarkdownNode],
    markup: &mut String,
    plain_text: &mut String,
    linkify: bool,
) {
    for node in nodes {
        match node {
            MarkdownNode::Text { text, .. } => {
                if linkify {
                    append_linkified_text(text, markup);
                } else {
                    markup.push_str(&pango_escape(text));
                }
                plain_text.push_str(text);
            }
            MarkdownNode::Element(element) => {
                append_inline_element(element, markup, plain_text, linkify)
            }
        }
    }
}

fn append_inline_element(
    element: &MarkdownElement,
    markup: &mut String,
    plain_text: &mut String,
    linkify: bool,
) {
    let child = inline_text_with_linkify(&element.children, linkify && element.tag != "a");
    match element.tag.as_str() {
        "strong" | "b" => {
            markup.push_str("<b>");
            markup.push_str(&child.markup);
            markup.push_str("</b>");
            plain_text.push_str(&child.plain_text);
        }
        "em" | "i" | "cite" | "var" => {
            markup.push_str("<i>");
            markup.push_str(&child.markup);
            markup.push_str("</i>");
            plain_text.push_str(&child.plain_text);
        }
        "s" | "del" => {
            markup.push_str("<span strikethrough=\"true\">");
            markup.push_str(&child.markup);
            markup.push_str("</span>");
            plain_text.push_str(&child.plain_text);
        }
        "code" | "kbd" => {
            markup.push_str("<span font_family=\"monospace\">");
            markup.push_str(&pango_escape(&child.plain_text));
            markup.push_str("</span>");
            plain_text.push_str(&child.plain_text);
        }
        "sup" => {
            markup.push_str("<span rise=\"6000\" size=\"smaller\">");
            markup.push_str(&child.markup);
            markup.push_str("</span>");
            plain_text.push_str(&child.plain_text);
        }
        "sub" => {
            markup.push_str("<span rise=\"-3000\" size=\"smaller\">");
            markup.push_str(&child.markup);
            markup.push_str("</span>");
            plain_text.push_str(&child.plain_text);
        }
        "a" => {
            if let Some(href) = attr_value(&element.attrs, "href").filter(|href| !href.is_empty()) {
                markup.push_str("<a href=\"");
                markup.push_str(&pango_escape_attribute(&href));
                markup.push_str("\">");
                markup.push_str(&child.markup);
                markup.push_str("</a>");
            } else {
                markup.push_str(&child.markup);
            }
            plain_text.push_str(&child.plain_text);
        }
        "br" => {
            markup.push('\n');
            plain_text.push('\n');
        }
        "img" => {
            let alt = attr_value(&element.attrs, "alt")
                .unwrap_or_else(|| text_content(&element.children));
            let source = attr_value(&element.attrs, "src").unwrap_or_default();
            let fallback = if alt.trim().is_empty() { source } else { alt };
            let placeholder = format!("[Image: {fallback}]");
            markup.push_str("<span alpha=\"65%\">");
            markup.push_str(&pango_escape(&placeholder));
            markup.push_str("</span>");
            plain_text.push_str(&placeholder);
        }
        "input" => {}
        _ => {
            markup.push_str(&child.markup);
            plain_text.push_str(&child.plain_text);
        }
    }
}

fn append_linkified_text(text: &str, markup: &mut String) {
    let mut cursor = 0;
    while let Some((start, end)) = next_autolink(text, cursor) {
        if cursor < start {
            markup.push_str(&pango_escape(&text[cursor..start]));
        }

        let url = &text[start..end];
        markup.push_str("<a href=\"");
        markup.push_str(&pango_escape_attribute(url));
        markup.push_str("\">");
        markup.push_str(&pango_escape(url));
        markup.push_str("</a>");
        cursor = end;
    }

    if cursor < text.len() {
        markup.push_str(&pango_escape(&text[cursor..]));
    }
}

fn next_autolink(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let rest = text.get(cursor..)?;
    let http = rest.find("http://").map(|index| cursor + index);
    let https = rest.find("https://").map(|index| cursor + index);
    let start = match (http, https) {
        (Some(http), Some(https)) => http.min(https),
        (Some(http), None) => http,
        (None, Some(https)) => https,
        (None, None) => return None,
    };

    let mut end = text[start..]
        .char_indices()
        .find_map(|(index, ch)| {
            (index > 0 && (ch.is_whitespace() || ch == '<')).then_some(start + index)
        })
        .unwrap_or(text.len());

    while end > start {
        let Some(ch) = text[..end].chars().next_back() else {
            break;
        };
        if matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}') {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }

    (end > start).then_some((start, end))
}

pub(super) fn block_text(blocks: &[MarkdownPreviewBlock]) -> RenderedText {
    let mut markup = Vec::new();
    let mut plain_text = Vec::new();
    for block in blocks {
        let text = match &block.kind {
            MarkdownPreviewBlockKind::Heading { text, .. }
            | MarkdownPreviewBlockKind::Paragraph { text, .. } => text.clone(),
            MarkdownPreviewBlockKind::Blockquote { text, alert } => {
                if let Some(alert) = alert {
                    RenderedText {
                        markup: format!("<b>{}</b>\n{}", alert.title(), text.markup),
                        plain_text: format!("{}\n{}", alert.title(), text.plain_text),
                    }
                } else {
                    text.clone()
                }
            }
            MarkdownPreviewBlockKind::List(items) => RenderedText {
                markup: items
                    .iter()
                    .map(|item| format!("{} {}", pango_escape(&item.marker), item.text.markup))
                    .collect::<Vec<_>>()
                    .join("\n"),
                plain_text: items
                    .iter()
                    .map(|item| format!("{} {}", item.marker, item.text.plain_text))
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
            MarkdownPreviewBlockKind::CodeBlock { code, .. } => RenderedText::plain(code),
            MarkdownPreviewBlockKind::ThematicBreak => RenderedText::plain("----------------"),
            MarkdownPreviewBlockKind::Table { headers, rows, .. } => RenderedText::plain(
                &std::iter::once(headers)
                    .chain(rows.iter())
                    .map(|row| {
                        row.iter()
                            .map(|cell| cell.plain_text.as_str())
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            MarkdownPreviewBlockKind::ImageGroup { images, .. } => RenderedText::plain(
                &images
                    .iter()
                    .map(|image| image_description(image))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        };
        if !text.plain_text.trim().is_empty() {
            markup.push(text.markup);
            plain_text.push(text.plain_text);
        }
    }
    RenderedText {
        markup: markup.join("\n"),
        plain_text: plain_text.join("\n"),
    }
}

fn image_description(image: &RenderedImageItem) -> String {
    [&image.alt, image.title.as_deref().unwrap_or_default()]
        .into_iter()
        .chain(std::iter::once(image.source.as_deref().unwrap_or_default()))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" - ")
}

impl RenderedText {
    fn plain(text: &str) -> Self {
        Self {
            markup: pango_escape(text),
            plain_text: text.to_string(),
        }
    }
}

impl MarkdownAlertKind {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }

    pub(super) fn css_class(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Tip => "tip",
            Self::Important => "important",
            Self::Warning => "warning",
            Self::Caution => "caution",
        }
    }
}

pub(super) fn pango_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) fn pango_escape_attribute(text: &str) -> String {
    pango_escape(text).replace('"', "&quot;")
}
