use adw::prelude::*;
use base64::Engine;
use gtk::{gdk, gio, pango};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

const DEFAULT_IMAGE_WIDTH: i32 = 320;
const MAX_IMAGE_WIDTH: i32 = 1200;
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
.craic-markdown-blockquote {
    border-left: 3px solid alpha(@view_fg_color, 0.25);
    padding-left: 12px;
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

#[derive(Clone, Debug)]
enum MarkdownNode {
    Text(String),
    Element(MarkdownElement),
}

#[derive(Clone, Debug)]
struct MarkdownElement {
    tag: String,
    attrs: Vec<HtmlAttribute>,
    children: Vec<MarkdownNode>,
}

#[derive(Clone, Debug)]
struct HtmlAttribute {
    name: String,
    value: String,
}

#[derive(Clone, Debug)]
struct ElementBuilder {
    tag: String,
    attrs: Vec<HtmlAttribute>,
    children: Vec<MarkdownNode>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HtmlTagKind {
    Start,
    End,
    SelfClosing,
}

#[derive(Clone, Debug)]
struct HtmlTag {
    kind: HtmlTagKind,
    name: String,
    attrs: Vec<HtmlAttribute>,
}

#[derive(Clone, Copy, Debug, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    monospace: bool,
    superscript: bool,
    subscript: bool,
    link: bool,
}

#[derive(Clone, Debug, Default)]
struct RenderContext {
    inline_style: InlineStyle,
    base_path: Option<PathBuf>,
    table_alignments: Vec<Vec<Alignment>>,
}

pub(in crate::ui) struct MarkdownPreview {
    pub(in crate::ui) root: gtk::ScrolledWindow,
    document: gtk::Box,
    stylesheet: gtk::CssProvider,
    document_stylesheet: gtk::CssProvider,
    base_path: RefCell<Option<PathBuf>>,
}

impl MarkdownPreview {
    pub(in crate::ui) fn new() -> Rc<Self> {
        install_default_css();

        let document = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .hexpand(true)
            .vexpand(true)
            .build();
        document.add_css_class("craic-markdown-document");

        let clamp = adw::Clamp::builder()
            .maximum_size(980)
            .tightening_threshold(720)
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .child(&document)
            .build();

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&clamp)
            .build();
        root.add_css_class("craic-markdown-preview");

        let stylesheet = gtk::CssProvider::new();
        let document_stylesheet = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &stylesheet,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &document_stylesheet,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }

        Rc::new(Self {
            root,
            document,
            stylesheet,
            document_stylesheet,
            base_path: RefCell::new(None),
        })
    }

    pub(in crate::ui) fn set_markdown(&self, markdown: &str) {
        self.set_markdown_with_base_path(markdown, None);
    }

    pub(in crate::ui) fn set_markdown_with_base_path(
        &self,
        markdown: &str,
        base_path: Option<&Path>,
    ) {
        self.base_path.replace(base_path.map(Path::to_path_buf));
        clear_box(&self.document);

        let nodes = parse_markdown(markdown);
        let context = RenderContext {
            base_path: self.base_path.borrow().clone(),
            ..RenderContext::default()
        };

        self.document_stylesheet
            .load_from_data(&collect_style_css(&nodes));

        for node in nodes {
            append_block_node(&self.document, &node, &context);
        }
    }

    pub(in crate::ui) fn set_stylesheet(&self, css: &str) {
        self.stylesheet.load_from_data(css);
    }
}

fn install_default_css() {
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

fn parse_markdown(markdown: &str) -> Vec<MarkdownNode> {
    let parser = Parser::new_ext(markdown, Options::all());
    let mut stack = vec![ElementBuilder::new("document", Vec::new())];

    for event in parser {
        match event {
            Event::Start(tag) => stack.push(markdown_tag_builder(tag)),
            Event::End(_) => finish_top_element(&mut stack),
            Event::Text(text) => append_text(&mut stack, text.as_ref()),
            Event::Code(code) => append_element_with_text(
                &mut stack,
                "code",
                vec![HtmlAttribute::new("class", "craic-markdown-code-inline")],
                code.as_ref(),
            ),
            Event::InlineMath(math) => append_element_with_text(
                &mut stack,
                "span",
                vec![HtmlAttribute::new("class", "math math-inline")],
                math.as_ref(),
            ),
            Event::DisplayMath(math) => append_element_with_text(
                &mut stack,
                "div",
                vec![HtmlAttribute::new("class", "math math-display")],
                math.as_ref(),
            ),
            Event::Html(html) => {
                if current_tag(&stack).is_some_and(|tag| tag == "pre") {
                    append_text(&mut stack, html.as_ref());
                } else {
                    append_html_fragment(&mut stack, html.as_ref());
                }
            }
            Event::InlineHtml(html) => append_inline_html(&mut stack, html.as_ref()),
            Event::FootnoteReference(reference) => append_element(
                &mut stack,
                MarkdownElement::new(
                    "sup",
                    vec![HtmlAttribute::new("class", "footnote-reference")],
                    vec![MarkdownNode::Text(reference.to_string())],
                ),
            ),
            Event::SoftBreak => append_text(&mut stack, " "),
            Event::HardBreak => append_element(&mut stack, MarkdownElement::empty("br")),
            Event::Rule => append_element(&mut stack, MarkdownElement::empty("hr")),
            Event::TaskListMarker(checked) => append_element(
                &mut stack,
                MarkdownElement::new(
                    "input",
                    vec![
                        HtmlAttribute::new("type", "checkbox"),
                        HtmlAttribute::new("disabled", "true"),
                        HtmlAttribute::new("checked", if checked { "true" } else { "false" }),
                    ],
                    Vec::new(),
                ),
            ),
        }
    }

    while stack.len() > 1 {
        finish_top_element(&mut stack);
    }

    stack.pop().map(|root| root.children).unwrap_or_default()
}

fn markdown_tag_builder(tag: Tag<'_>) -> ElementBuilder {
    match tag {
        Tag::Paragraph => ElementBuilder::new("p", Vec::new()),
        Tag::Heading {
            level,
            id,
            classes,
            attrs,
        } => {
            let mut attributes = attrs
                .into_iter()
                .map(|(name, value)| {
                    HtmlAttribute::new(name.as_ref(), value.as_deref().unwrap_or(""))
                })
                .collect::<Vec<_>>();
            if let Some(id) = id {
                attributes.push(HtmlAttribute::new("id", id.as_ref()));
            }
            if !classes.is_empty() {
                attributes.push(HtmlAttribute::new(
                    "class",
                    &classes
                        .iter()
                        .map(|class| class.as_ref())
                        .collect::<Vec<_>>()
                        .join(" "),
                ));
            }
            ElementBuilder::new(heading_tag(level), attributes)
        }
        Tag::BlockQuote(kind) => {
            let class = kind
                .map(|kind| {
                    format!(
                        "markdown-alert-{}",
                        format!("{kind:?}").to_ascii_lowercase()
                    )
                })
                .unwrap_or_default();
            ElementBuilder::new("blockquote", class_attr(class))
        }
        Tag::CodeBlock(kind) => {
            let language = match kind {
                CodeBlockKind::Fenced(info) => info
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                CodeBlockKind::Indented => String::new(),
            };
            let mut attrs = Vec::new();
            if !language.is_empty() {
                attrs.push(HtmlAttribute::new("class", &format!("language-{language}")));
                attrs.push(HtmlAttribute::new("data-language", &language));
            }
            ElementBuilder::new("pre", attrs)
        }
        Tag::HtmlBlock => ElementBuilder::new("div", class_attr("html-block")),
        Tag::List(start) => {
            let mut attrs = Vec::new();
            if let Some(start) = start {
                attrs.push(HtmlAttribute::new("start", &start.to_string()));
            }
            ElementBuilder::new(if start.is_some() { "ol" } else { "ul" }, attrs)
        }
        Tag::Item => ElementBuilder::new("li", Vec::new()),
        Tag::FootnoteDefinition(name) => ElementBuilder::new(
            "div",
            vec![
                HtmlAttribute::new("class", "footnote-definition"),
                HtmlAttribute::new("id", name.as_ref()),
            ],
        ),
        Tag::DefinitionList => ElementBuilder::new("dl", Vec::new()),
        Tag::DefinitionListTitle => ElementBuilder::new("dt", Vec::new()),
        Tag::DefinitionListDefinition => ElementBuilder::new("dd", Vec::new()),
        Tag::Table(alignments) => ElementBuilder::new(
            "table",
            vec![HtmlAttribute::new(
                "data-alignments",
                &alignment_attr(&alignments),
            )],
        ),
        Tag::TableHead => ElementBuilder::new("thead", Vec::new()),
        Tag::TableRow => ElementBuilder::new("tr", Vec::new()),
        Tag::TableCell => ElementBuilder::new("td", Vec::new()),
        Tag::Emphasis => ElementBuilder::new("em", Vec::new()),
        Tag::Strong => ElementBuilder::new("strong", Vec::new()),
        Tag::Strikethrough => ElementBuilder::new("s", Vec::new()),
        Tag::Superscript => ElementBuilder::new("sup", Vec::new()),
        Tag::Subscript => ElementBuilder::new("sub", Vec::new()),
        Tag::Link {
            dest_url,
            title,
            id,
            ..
        } => ElementBuilder::new(
            "a",
            vec![
                HtmlAttribute::new("href", dest_url.as_ref()),
                HtmlAttribute::new("title", title.as_ref()),
                HtmlAttribute::new("id", id.as_ref()),
            ],
        ),
        Tag::Image {
            dest_url,
            title,
            id,
            ..
        } => ElementBuilder::new(
            "img",
            vec![
                HtmlAttribute::new("src", dest_url.as_ref()),
                HtmlAttribute::new("title", title.as_ref()),
                HtmlAttribute::new("id", id.as_ref()),
            ],
        ),
        Tag::MetadataBlock(_) => ElementBuilder::new("div", class_attr("metadata")),
    }
}

fn heading_tag(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "h1",
        HeadingLevel::H2 => "h2",
        HeadingLevel::H3 => "h3",
        HeadingLevel::H4 => "h4",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    }
}

fn alignment_attr(alignments: &[Alignment]) -> String {
    alignments
        .iter()
        .map(|alignment| match alignment {
            Alignment::None => "none",
            Alignment::Left => "left",
            Alignment::Center => "center",
            Alignment::Right => "right",
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn class_attr(class: impl AsRef<str>) -> Vec<HtmlAttribute> {
    let class = class.as_ref();
    if class.is_empty() {
        Vec::new()
    } else {
        vec![HtmlAttribute::new("class", class)]
    }
}

fn append_html_fragment(stack: &mut Vec<ElementBuilder>, html: &str) {
    for node in parse_html_fragment(html) {
        append_node(stack, node);
    }
}

fn append_inline_html(stack: &mut Vec<ElementBuilder>, html: &str) {
    match parse_single_html_tag(html) {
        Some(tag) => apply_html_tag(stack, tag),
        None => append_text(stack, html),
    }
}

fn parse_html_fragment(html: &str) -> Vec<MarkdownNode> {
    let mut stack = vec![ElementBuilder::new("document", Vec::new())];
    let mut cursor = 0;

    while cursor < html.len() {
        let Some(relative_tag_start) = html[cursor..].find('<') else {
            append_text(&mut stack, &html[cursor..]);
            break;
        };

        let tag_start = cursor + relative_tag_start;
        if cursor < tag_start {
            append_text(&mut stack, &html[cursor..tag_start]);
        }

        match parse_html_tag_at(html, tag_start) {
            Some((tag, next)) => {
                apply_html_tag(&mut stack, tag);
                cursor = next;
            }
            None => {
                append_text(&mut stack, "<");
                cursor = tag_start + 1;
            }
        }
    }

    while stack.len() > 1 {
        finish_top_element(&mut stack);
    }

    stack.pop().map(|root| root.children).unwrap_or_default()
}

fn parse_single_html_tag(html: &str) -> Option<HtmlTag> {
    let trimmed = html.trim();
    let (tag, next) = parse_html_tag_at(trimmed, 0)?;
    (next == trimmed.len()).then_some(tag)
}

fn parse_html_tag_at(html: &str, start: usize) -> Option<(HtmlTag, usize)> {
    if !html[start..].starts_with('<') {
        return None;
    }
    if html[start..].starts_with("<!--") {
        let end = html[start + 4..].find("-->")? + start + 7;
        return Some((
            HtmlTag {
                kind: HtmlTagKind::SelfClosing,
                name: "comment".to_string(),
                attrs: Vec::new(),
            },
            end,
        ));
    }

    let mut quote = None;
    let mut end = None;
    for (offset, ch) in html[start + 1..].char_indices() {
        match ch {
            '"' | '\'' if quote == Some(ch) => quote = None,
            '"' | '\'' if quote.is_none() => quote = Some(ch),
            '>' if quote.is_none() => {
                end = Some(start + 1 + offset);
                break;
            }
            _ => {}
        }
    }

    let end = end?;
    let content = html[start + 1..end].trim();
    parse_html_tag_content(content).map(|tag| (tag, end + 1))
}

fn parse_html_tag_content(content: &str) -> Option<HtmlTag> {
    if content.is_empty()
        || content.starts_with('!')
        || content.starts_with('?')
        || content.starts_with("!--")
    {
        return Some(HtmlTag {
            kind: HtmlTagKind::SelfClosing,
            name: "comment".to_string(),
            attrs: Vec::new(),
        });
    }

    if let Some(rest) = content.strip_prefix('/') {
        let name = read_html_name(rest.trim_start())?;
        return Some(HtmlTag {
            kind: HtmlTagKind::End,
            name: name.to_ascii_lowercase(),
            attrs: Vec::new(),
        });
    }

    let self_closing = content.ends_with('/');
    let content = content.strip_suffix('/').unwrap_or(content).trim_end();
    let name = read_html_name(content)?;
    let attrs = parse_html_attrs(&content[name.len()..]);

    Some(HtmlTag {
        kind: if self_closing || is_void_html_tag(name) {
            HtmlTagKind::SelfClosing
        } else {
            HtmlTagKind::Start
        },
        name: name.to_ascii_lowercase(),
        attrs,
    })
}

fn read_html_name(input: &str) -> Option<&str> {
    let end = input
        .char_indices()
        .find_map(|(index, ch)| {
            (!(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':'))).then_some(index)
        })
        .unwrap_or(input.len());
    (end > 0).then_some(&input[..end])
}

fn parse_html_attrs(input: &str) -> Vec<HtmlAttribute> {
    let mut attrs = Vec::new();
    let mut cursor = 0;

    while cursor < input.len() {
        cursor += input[cursor..]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        if cursor >= input.len() {
            break;
        }

        let Some(name) = read_html_name(&input[cursor..]) else {
            break;
        };
        cursor += name.len();
        cursor += input[cursor..]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();

        let value = if input[cursor..].starts_with('=') {
            cursor += 1;
            cursor += input[cursor..]
                .chars()
                .take_while(|ch| ch.is_whitespace())
                .map(char::len_utf8)
                .sum::<usize>();
            read_attr_value(input, &mut cursor)
        } else {
            String::new()
        };

        attrs.push(HtmlAttribute::new(&name.to_ascii_lowercase(), &value));
    }

    attrs
}

fn read_attr_value(input: &str, cursor: &mut usize) -> String {
    let Some(first) = input[*cursor..].chars().next() else {
        return String::new();
    };

    if first == '"' || first == '\'' {
        *cursor += first.len_utf8();
        let start = *cursor;
        while *cursor < input.len() {
            let ch = input[*cursor..].chars().next().unwrap_or_default();
            if ch == first {
                let value = html_unescape(&input[start..*cursor]);
                *cursor += ch.len_utf8();
                return value;
            }
            *cursor += ch.len_utf8();
        }
        html_unescape(&input[start..])
    } else {
        let start = *cursor;
        while *cursor < input.len() {
            let ch = input[*cursor..].chars().next().unwrap_or_default();
            if ch.is_whitespace() || ch == '>' {
                break;
            }
            *cursor += ch.len_utf8();
        }
        html_unescape(&input[start..*cursor])
    }
}

fn html_unescape(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn apply_html_tag(stack: &mut Vec<ElementBuilder>, tag: HtmlTag) {
    if tag.name == "comment" {
        return;
    }

    match tag.kind {
        HtmlTagKind::Start => stack.push(ElementBuilder::new(&tag.name, tag.attrs)),
        HtmlTagKind::SelfClosing => append_element(
            stack,
            MarkdownElement::new(&tag.name, tag.attrs, Vec::new()),
        ),
        HtmlTagKind::End => finish_matching_html_element(stack, &tag.name),
    }
}

fn finish_matching_html_element(stack: &mut Vec<ElementBuilder>, name: &str) {
    let Some(index) = stack
        .iter()
        .rposition(|builder| builder.tag.eq_ignore_ascii_case(name))
    else {
        return;
    };

    while stack.len() > index {
        finish_top_element(stack);
    }
}

fn is_void_html_tag(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn append_text(stack: &mut [ElementBuilder], text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(MarkdownNode::Text(previous)) =
        stack.last_mut().and_then(|top| top.children.last_mut())
    {
        previous.push_str(text);
    } else if let Some(top) = stack.last_mut() {
        top.children.push(MarkdownNode::Text(text.to_string()));
    }
}

fn append_element_with_text(
    stack: &mut Vec<ElementBuilder>,
    tag: &str,
    attrs: Vec<HtmlAttribute>,
    text: &str,
) {
    append_element(
        stack,
        MarkdownElement::new(tag, attrs, vec![MarkdownNode::Text(text.to_string())]),
    );
}

fn append_node(stack: &mut [ElementBuilder], node: MarkdownNode) {
    if let Some(top) = stack.last_mut() {
        top.children.push(node);
    }
}

fn append_element(stack: &mut [ElementBuilder], element: MarkdownElement) {
    append_node(stack, MarkdownNode::Element(element));
}

fn finish_top_element(stack: &mut Vec<ElementBuilder>) {
    if stack.len() <= 1 {
        return;
    }

    let mut builder = stack.pop().expect("markdown element stack is not empty");
    if builder.tag == "img" && attr_value(&builder.attrs, "alt").is_none() {
        let alt = text_content(&builder.children);
        if !alt.is_empty() {
            builder.attrs.push(HtmlAttribute::new("alt", &alt));
        }
        builder.children.clear();
    }

    append_element(
        stack,
        MarkdownElement::new(&builder.tag, builder.attrs, builder.children),
    );
}

fn current_tag(stack: &[ElementBuilder]) -> Option<&str> {
    stack.last().map(|top| top.tag.as_str())
}

fn append_block_node(parent: &gtk::Box, node: &MarkdownNode, context: &RenderContext) {
    match node {
        MarkdownNode::Text(text) => {
            if !text.trim().is_empty() {
                let element = MarkdownElement::new(
                    "p",
                    Vec::new(),
                    vec![MarkdownNode::Text(text.trim().to_string())],
                );
                parent.append(&render_flow_element(&element, context));
            }
        }
        MarkdownNode::Element(element) => {
            let widget = render_block_element(element, context);
            parent.append(&widget);
        }
    }
}

fn render_block_element(element: &MarkdownElement, context: &RenderContext) -> gtk::Widget {
    match element.tag.as_str() {
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "dt" | "dd" => {
            render_flow_element(element, context).upcast()
        }
        "pre" => render_pre(element).upcast(),
        "blockquote" => render_container(element, context).upcast(),
        "ul" | "ol" => render_list(element, context).upcast(),
        "li" => render_list_item(element, context, None).upcast(),
        "table" => render_table(element, context).upcast(),
        "script" | "style" => gtk::Box::builder().visible(false).build().upcast(),
        "hr" => gtk::Separator::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build()
            .upcast(),
        "img" => render_image_block(element, context).upcast(),
        "br" => gtk::Separator::builder()
            .orientation(gtk::Orientation::Horizontal)
            .opacity(0.0)
            .height_request(1)
            .build()
            .upcast(),
        _ if is_inline_tag(&element.tag) || element_has_inline_children(element) => {
            render_flow_element(element, context).upcast()
        }
        _ => render_container(element, context).upcast(),
    }
}

fn render_flow_element(element: &MarkdownElement, context: &RenderContext) -> gtk::FlowBox {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(false)
        .min_children_per_line(1)
        .max_children_per_line(u32::MAX)
        .row_spacing(4)
        .column_spacing(4)
        .halign(halign_for_attrs(&element.attrs))
        .hexpand(true)
        .build();
    apply_element_classes(
        flow.upcast_ref(),
        element,
        Some(&format!("craic-markdown-{}", element.tag)),
    );

    let mut inline_context = context.clone();
    apply_inline_tag_style(&element.tag, &mut inline_context.inline_style);
    render_inline_children(&flow, &element.children, &inline_context);
    flow
}

fn render_inline_children(flow: &gtk::FlowBox, nodes: &[MarkdownNode], context: &RenderContext) {
    for node in nodes {
        match node {
            MarkdownNode::Text(text) => append_text_runs(flow, text, context.inline_style),
            MarkdownNode::Element(element) => render_inline_element(flow, element, context),
        }
    }
}

fn render_inline_element(flow: &gtk::FlowBox, element: &MarkdownElement, context: &RenderContext) {
    match element.tag.as_str() {
        "img" => flow.append(&render_image_inline(element, context)),
        "br" => flow.append(&gtk::Label::new(Some("\n"))),
        "input" => flow.append(&render_input(element)),
        "code" => flow.append(&render_inline_label(
            &text_content(&element.children),
            with_inline_style(context.inline_style, |style| style.monospace = true),
            element,
            true,
        )),
        tag if is_inline_tag(tag) => {
            let mut nested = context.clone();
            apply_inline_tag_style(tag, &mut nested.inline_style);
            if tag == "a" {
                nested.inline_style.link = true;
            }
            render_inline_children(flow, &element.children, &nested);
        }
        _ if element_has_inline_children(element) => {
            let mut nested = context.clone();
            apply_inline_tag_style(&element.tag, &mut nested.inline_style);
            render_inline_children(flow, &element.children, &nested);
        }
        _ => flow.append(&render_block_element(element, context)),
    }
}

fn append_text_runs(flow: &gtk::FlowBox, text: &str, style: InlineStyle) {
    for word in text.split_whitespace() {
        flow.append(&render_text_label(word, style));
    }
}

fn render_text_label(text: &str, style: InlineStyle) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .selectable(true)
        .halign(gtk::Align::Start)
        .wrap(false)
        .build();
    label.add_css_class("craic-markdown-text");
    apply_text_attrs(&label, style);
    label
}

fn render_inline_label(
    text: &str,
    style: InlineStyle,
    element: &MarkdownElement,
    preserve_spaces: bool,
) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(if preserve_spaces { text } else { text.trim() })
        .selectable(true)
        .halign(gtk::Align::Start)
        .wrap(false)
        .build();
    apply_element_classes(
        label.upcast_ref(),
        element,
        Some("craic-markdown-code-inline"),
    );
    apply_text_attrs(&label, style);
    label
}

fn apply_text_attrs(label: &gtk::Label, style: InlineStyle) {
    let attrs = pango::AttrList::new();
    let len = label.text().len() as u32;

    if style.bold {
        insert_text_attr(&attrs, pango::AttrInt::new_weight(pango::Weight::Bold), len);
    }
    if style.italic {
        insert_text_attr(&attrs, pango::AttrInt::new_style(pango::Style::Italic), len);
    }
    if style.strikethrough {
        insert_text_attr(&attrs, pango::AttrInt::new_strikethrough(true), len);
    }
    if style.superscript {
        insert_text_attr(&attrs, pango::AttrInt::new_rise(6000), len);
        insert_text_attr(&attrs, pango::AttrSize::new(8 * pango::SCALE), len);
    }
    if style.subscript {
        insert_text_attr(&attrs, pango::AttrInt::new_rise(-3000), len);
        insert_text_attr(&attrs, pango::AttrSize::new(8 * pango::SCALE), len);
    }
    if style.link {
        insert_text_attr(
            &attrs,
            pango::AttrInt::new_underline(pango::Underline::Single),
            len,
        );
    }

    label.set_attributes(Some(&attrs));
    if style.monospace {
        label.add_css_class("monospace");
    }
    if style.link {
        label.add_css_class("craic-markdown-a");
    }
}

fn insert_text_attr<T>(attrs: &pango::AttrList, attr: T, len: u32)
where
    T: Into<pango::Attribute>,
{
    let mut attr = attr.into();
    attr.set_start_index(0);
    attr.set_end_index(len);
    attrs.insert(attr);
}

fn render_pre(element: &MarkdownElement) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text_content(&element.children).as_str())
        .selectable(true)
        .halign(gtk::Align::Fill)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .build();
    apply_element_classes(label.upcast_ref(), element, Some("craic-markdown-pre"));
    label
}

fn render_container(element: &MarkdownElement, context: &RenderContext) -> gtk::Box {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .hexpand(true)
        .halign(halign_for_attrs(&element.attrs))
        .build();
    apply_element_classes(
        container.upcast_ref(),
        element,
        Some(&format!("craic-markdown-{}", element.tag)),
    );

    for child in &element.children {
        append_block_node(&container, child, context);
    }

    container
}

fn render_list(element: &MarkdownElement, context: &RenderContext) -> gtk::Box {
    let list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .build();
    apply_element_classes(
        list.upcast_ref(),
        element,
        Some(&format!("craic-markdown-{}", element.tag)),
    );

    let start = attr_value(&element.attrs, "start")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1);
    let ordered_context = context.clone();

    let mut index = start;
    for child in &element.children {
        match child {
            MarkdownNode::Element(item) if item.tag == "li" && element.tag == "ol" => {
                list.append(&render_list_item(item, &ordered_context, Some(index)));
                index += 1;
            }
            MarkdownNode::Element(item) if item.tag == "li" => {
                list.append(&render_list_item(item, &ordered_context, None));
            }
            _ => append_block_node(&list, child, &ordered_context),
        }
    }

    list
}

fn render_list_item(
    element: &MarkdownElement,
    context: &RenderContext,
    ordered_index: Option<u64>,
) -> gtk::Box {
    let marker = gtk::Label::builder()
        .label(
            ordered_index
                .map(|index| format!("{index}."))
                .unwrap_or_else(|| "•".to_string())
                .as_str(),
        )
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .width_chars(3)
        .build();
    marker.add_css_class("craic-markdown-list-marker");

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .build();
    for child in &element.children {
        append_block_node(&body, child, context);
    }

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .hexpand(true)
        .build();
    apply_element_classes(row.upcast_ref(), element, Some("craic-markdown-li"));
    row.append(&marker);
    row.append(&body);
    row
}

fn render_table(element: &MarkdownElement, context: &RenderContext) -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .column_spacing(0)
        .row_spacing(0)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build();
    apply_element_classes(grid.upcast_ref(), element, Some("craic-markdown-table"));

    let alignments = attr_value(&element.attrs, "data-alignments")
        .as_deref()
        .map(parse_alignment_attr)
        .unwrap_or_default();
    let mut table_context = context.clone();
    table_context.table_alignments.push(alignments);

    let mut row_index = 0;
    append_table_rows(&grid, &element.children, &table_context, &mut row_index);
    grid
}

fn append_table_rows(
    grid: &gtk::Grid,
    nodes: &[MarkdownNode],
    context: &RenderContext,
    row_index: &mut i32,
) {
    for node in nodes {
        if let MarkdownNode::Element(element) = node {
            match element.tag.as_str() {
                "thead" | "tbody" => append_table_rows(grid, &element.children, context, row_index),
                "tr" => {
                    append_table_row(grid, element, context, *row_index);
                    *row_index += 1;
                }
                _ => {}
            }
        }
    }
}

fn append_table_row(
    grid: &gtk::Grid,
    row: &MarkdownElement,
    context: &RenderContext,
    row_index: i32,
) {
    let mut column = 0;
    for child in &row.children {
        let MarkdownNode::Element(cell) = child else {
            continue;
        };
        if cell.tag != "td" && cell.tag != "th" {
            continue;
        }

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .halign(table_cell_halign(context, column))
            .build();
        apply_element_classes(body.upcast_ref(), cell, Some("craic-markdown-table-cell"));
        for node in &cell.children {
            append_block_node(&body, node, context);
        }

        grid.attach(&body, column, row_index, 1, 1);
        column += 1;
    }
}

fn table_cell_halign(context: &RenderContext, column: i32) -> gtk::Align {
    context
        .table_alignments
        .last()
        .and_then(|alignments| alignments.get(column as usize))
        .map(|alignment| match alignment {
            Alignment::None | Alignment::Left => gtk::Align::Start,
            Alignment::Center => gtk::Align::Center,
            Alignment::Right => gtk::Align::End,
        })
        .unwrap_or(gtk::Align::Start)
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

fn render_image_block(element: &MarkdownElement, context: &RenderContext) -> gtk::Box {
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(halign_for_attrs(&element.attrs))
        .hexpand(true)
        .build();
    box_.append(&render_image_inline(element, context));
    box_
}

fn render_image_inline(element: &MarkdownElement, context: &RenderContext) -> gtk::Widget {
    let alt = attr_value(&element.attrs, "alt").unwrap_or_else(|| text_content(&element.children));
    let Some(src) = attr_value(&element.attrs, "src") else {
        return unresolved_image_label(&alt, "missing image source").upcast();
    };

    if let Some(texture) = texture_from_data_uri(&src) {
        let picture = image_picture(element, &alt);
        picture.set_paintable(Some(&texture));
        return picture.upcast();
    }

    let Some(file) = image_file_for_src(&src, context.base_path.as_deref()) else {
        return unresolved_image_label(&alt, &src).upcast();
    };

    let picture = image_picture(element, &alt);
    picture.set_file(Some(&file));
    picture.upcast()
}

fn image_picture(element: &MarkdownElement, alt: &str) -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .alternative_text(alt)
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .halign(halign_for_attrs(&element.attrs))
        .valign(gtk::Align::Center)
        .build();
    apply_element_classes(picture.upcast_ref(), element, Some("craic-markdown-img"));

    let width = attr_value(&element.attrs, "width")
        .and_then(|value| parse_css_length_px(&value))
        .or_else(|| {
            style_value(&element.attrs, "width").and_then(|value| parse_css_length_px(&value))
        })
        .unwrap_or(DEFAULT_IMAGE_WIDTH)
        .clamp(1, MAX_IMAGE_WIDTH);
    picture.set_size_request(width, -1);

    if let Some(height) = attr_value(&element.attrs, "height")
        .and_then(|value| parse_css_length_px(&value))
        .or_else(|| {
            style_value(&element.attrs, "height").and_then(|value| parse_css_length_px(&value))
        })
    {
        picture.set_size_request(width, height.max(1));
    }

    picture
}

fn unresolved_image_label(alt: &str, detail: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(&format!(
            "[image: {}]",
            if alt.trim().is_empty() { detail } else { alt }
        ))
        .halign(gtk::Align::Start)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .build();
    label.add_css_class("craic-markdown-img-unresolved");
    label
}

fn texture_from_data_uri(src: &str) -> Option<gdk::Texture> {
    let (_, data) = src.split_once(";base64,")?;
    if !src[..src.find(";base64,")?].starts_with("data:image/") {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(bytes)).ok()
}

fn image_file_for_src(src: &str, base_path: Option<&Path>) -> Option<gio::File> {
    if src.starts_with("file://") {
        return Some(gio::File::for_uri(src));
    }
    if src.contains("://") || src.starts_with("data:") {
        return None;
    }

    let path = Path::new(src);
    if path.is_absolute() {
        Some(gio::File::for_path(path))
    } else {
        base_path.map(|base_path| gio::File::for_path(base_path.join(path)))
    }
}

fn render_input(element: &MarkdownElement) -> gtk::CheckButton {
    let input = gtk::CheckButton::builder()
        .active(attr_value(&element.attrs, "checked").is_some_and(|value| value == "true"))
        .sensitive(false)
        .build();
    apply_element_classes(input.upcast_ref(), element, Some("craic-markdown-input"));
    input
}

fn apply_element_classes(
    widget: &gtk::Widget,
    element: &MarkdownElement,
    base_class: Option<&str>,
) {
    if let Some(base_class) = base_class {
        widget.add_css_class(base_class);
    }
    widget.add_css_class("craic-markdown-element");

    for class in attr_value(&element.attrs, "class")
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(sanitize_css_class)
    {
        widget.add_css_class(&class);
    }
}

fn sanitize_css_class(class: &str) -> Option<String> {
    let class = class.trim();
    if class.is_empty() {
        return None;
    }

    Some(
        class
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '-'
                }
            })
            .collect(),
    )
}

fn halign_for_attrs(attrs: &[HtmlAttribute]) -> gtk::Align {
    attr_value(attrs, "align")
        .or_else(|| style_value(attrs, "text-align"))
        .or_else(|| style_value(attrs, "float"))
        .or_else(|| style_value(attrs, "justify-content"))
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "center" => gtk::Align::Center,
            "right" | "end" => gtk::Align::End,
            _ => gtk::Align::Start,
        })
        .unwrap_or(gtk::Align::Start)
}

fn style_value(attrs: &[HtmlAttribute], key: &str) -> Option<String> {
    let style = attr_value(attrs, "style")?;
    for declaration in style.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(key) {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn parse_css_length_px(value: &str) -> Option<i32> {
    let value = value.trim();
    if value.ends_with('%') {
        return None;
    }

    let number = value.strip_suffix("px").unwrap_or(value).trim();
    number
        .parse::<f64>()
        .ok()
        .map(|value| value.round().clamp(1.0, MAX_IMAGE_WIDTH as f64) as i32)
}

fn element_has_inline_children(element: &MarkdownElement) -> bool {
    element.children.iter().all(|child| match child {
        MarkdownNode::Text(_) => true,
        MarkdownNode::Element(child) => is_inline_tag(&child.tag) || child.tag == "img",
    })
}

fn is_inline_tag(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "abbr"
            | "b"
            | "cite"
            | "code"
            | "del"
            | "em"
            | "i"
            | "ins"
            | "kbd"
            | "mark"
            | "q"
            | "s"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "u"
            | "var"
    )
}

fn apply_inline_tag_style(tag: &str, style: &mut InlineStyle) {
    match tag {
        "b" | "strong" => style.bold = true,
        "cite" | "em" | "i" | "var" => style.italic = true,
        "code" | "kbd" => style.monospace = true,
        "del" | "s" => style.strikethrough = true,
        "sup" => style.superscript = true,
        "sub" => style.subscript = true,
        _ => {}
    }
}

fn with_inline_style<F>(mut style: InlineStyle, update: F) -> InlineStyle
where
    F: FnOnce(&mut InlineStyle),
{
    update(&mut style);
    style
}

fn attr_value(attrs: &[HtmlAttribute], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|attr| attr.name.eq_ignore_ascii_case(name))
        .map(|attr| attr.value.clone())
}

fn text_content(nodes: &[MarkdownNode]) -> String {
    let mut text = String::new();
    for node in nodes {
        match node {
            MarkdownNode::Text(value) => text.push_str(value),
            MarkdownNode::Element(element) => text.push_str(&text_content(&element.children)),
        }
    }
    text
}

fn collect_style_css(nodes: &[MarkdownNode]) -> String {
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

fn clear_box(box_: &gtk::Box) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child);
    }
}

impl ElementBuilder {
    fn new(tag: &str, attrs: Vec<HtmlAttribute>) -> Self {
        Self {
            tag: tag.to_ascii_lowercase(),
            attrs,
            children: Vec::new(),
        }
    }
}

impl MarkdownElement {
    fn new(tag: &str, attrs: Vec<HtmlAttribute>, children: Vec<MarkdownNode>) -> Self {
        Self {
            tag: tag.to_ascii_lowercase(),
            attrs,
            children,
        }
    }

    fn empty(tag: &str) -> Self {
        Self::new(tag, Vec::new(), Vec::new())
    }
}

impl HtmlAttribute {
    fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_ascii_lowercase(),
            value: value.to_string(),
        }
    }
}
