use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

use super::ast::{
    ElementBuilder, HtmlAttribute, MarkdownElement, MarkdownNode, append_element,
    append_element_with_text, append_text_source, current_tag, finish_top_element_source,
};
use super::html::{append_html_fragment, append_inline_html};

pub(super) fn parse_markdown(markdown: &str) -> Vec<MarkdownNode> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_DEFINITION_LIST);
    options.insert(Options::ENABLE_SUPERSCRIPT);
    options.insert(Options::ENABLE_SUBSCRIPT);

    let parser = Parser::new_ext(markdown, options).into_offset_iter();
    let mut stack = vec![ElementBuilder::new("document", Vec::new())];

    for (event, source) in parser {
        match event {
            Event::Start(tag) => stack.push(markdown_tag_builder(tag, Some(source))),
            Event::End(_) => finish_top_element_source(&mut stack, Some(source)),
            Event::Text(text) => append_text_source(&mut stack, text.as_ref(), Some(source)),
            Event::Code(code) => append_element_with_text(
                &mut stack,
                "code",
                vec![HtmlAttribute::new("class", "craic-markdown-code-inline")],
                code.as_ref(),
                Some(source),
            ),
            Event::InlineMath(math) => append_element_with_text(
                &mut stack,
                "span",
                vec![HtmlAttribute::new("class", "math math-inline")],
                math.as_ref(),
                Some(source),
            ),
            Event::DisplayMath(math) => append_element_with_text(
                &mut stack,
                "div",
                vec![HtmlAttribute::new("class", "math math-display")],
                math.as_ref(),
                Some(source),
            ),
            Event::Html(html) => {
                if current_tag(&stack).is_some_and(|tag| tag == "pre") {
                    append_text_source(&mut stack, html.as_ref(), Some(source));
                } else {
                    append_html_fragment(&mut stack, html.as_ref(), source.start);
                }
            }
            Event::InlineHtml(html) => append_inline_html(&mut stack, html.as_ref(), source.start),
            Event::FootnoteReference(reference) => append_element(
                &mut stack,
                MarkdownElement::with_source(
                    "sup",
                    vec![HtmlAttribute::new("class", "footnote-reference")],
                    vec![MarkdownNode::Text {
                        text: reference.to_string(),
                        source: Some(source.clone()),
                    }],
                    Some(source),
                ),
            ),
            Event::SoftBreak => append_text_source(&mut stack, " ", Some(source)),
            Event::HardBreak => {
                append_element(
                    &mut stack,
                    MarkdownElement::empty_with_source("br", Some(source)),
                );
            }
            Event::Rule => {
                append_element(
                    &mut stack,
                    MarkdownElement::empty_with_source("hr", Some(source)),
                );
            }
            Event::TaskListMarker(checked) => append_element(
                &mut stack,
                MarkdownElement::with_source(
                    "input",
                    vec![
                        HtmlAttribute::new("type", "checkbox"),
                        HtmlAttribute::new("disabled", "true"),
                        HtmlAttribute::new("checked", if checked { "true" } else { "false" }),
                    ],
                    Vec::new(),
                    Some(source),
                ),
            ),
        }
    }

    while stack.len() > 1 {
        finish_top_element_source(&mut stack, None);
    }

    stack
        .pop()
        .map(ElementBuilder::into_children)
        .unwrap_or_default()
}

fn markdown_tag_builder(tag: Tag<'_>, source: Option<std::ops::Range<usize>>) -> ElementBuilder {
    match tag {
        Tag::Paragraph => ElementBuilder::with_source("p", Vec::new(), source),
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
            ElementBuilder::with_source(heading_tag(level), attributes, source)
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
            ElementBuilder::with_source("blockquote", class_attr(class), source)
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
            ElementBuilder::with_source("pre", attrs, source)
        }
        Tag::HtmlBlock => ElementBuilder::with_source("div", class_attr("html-block"), source),
        Tag::List(start) => {
            let mut attrs = Vec::new();
            if let Some(start) = start {
                attrs.push(HtmlAttribute::new("start", &start.to_string()));
            }
            ElementBuilder::with_source(if start.is_some() { "ol" } else { "ul" }, attrs, source)
        }
        Tag::Item => ElementBuilder::with_source("li", Vec::new(), source),
        Tag::FootnoteDefinition(name) => ElementBuilder::with_source(
            "div",
            vec![
                HtmlAttribute::new("class", "footnote-definition"),
                HtmlAttribute::new("id", name.as_ref()),
            ],
            source,
        ),
        Tag::DefinitionList => ElementBuilder::with_source("dl", Vec::new(), source),
        Tag::DefinitionListTitle => ElementBuilder::with_source("dt", Vec::new(), source),
        Tag::DefinitionListDefinition => ElementBuilder::with_source("dd", Vec::new(), source),
        Tag::Table(alignments) => ElementBuilder::with_source(
            "table",
            vec![HtmlAttribute::new(
                "data-alignments",
                &alignment_attr(&alignments),
            )],
            source,
        ),
        Tag::TableHead => ElementBuilder::with_source("thead", Vec::new(), source),
        Tag::TableRow => ElementBuilder::with_source("tr", Vec::new(), source),
        Tag::TableCell => ElementBuilder::with_source("td", Vec::new(), source),
        Tag::Emphasis => ElementBuilder::with_source("em", Vec::new(), source),
        Tag::Strong => ElementBuilder::with_source("strong", Vec::new(), source),
        Tag::Strikethrough => ElementBuilder::with_source("s", Vec::new(), source),
        Tag::Superscript => ElementBuilder::with_source("sup", Vec::new(), source),
        Tag::Subscript => ElementBuilder::with_source("sub", Vec::new(), source),
        Tag::Link {
            dest_url,
            title,
            id,
            ..
        } => ElementBuilder::with_source(
            "a",
            vec![
                HtmlAttribute::new("href", dest_url.as_ref()),
                HtmlAttribute::new("title", title.as_ref()),
                HtmlAttribute::new("id", id.as_ref()),
            ],
            source,
        ),
        Tag::Image {
            dest_url,
            title,
            id,
            ..
        } => ElementBuilder::with_source(
            "img",
            vec![
                HtmlAttribute::new("src", dest_url.as_ref()),
                HtmlAttribute::new("title", title.as_ref()),
                HtmlAttribute::new("id", id.as_ref()),
            ],
            source,
        ),
        Tag::MetadataBlock(_) => ElementBuilder::with_source("div", class_attr("metadata"), source),
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
