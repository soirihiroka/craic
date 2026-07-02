use super::ast::{
    ElementBuilder, HtmlAttribute, MarkdownElement, MarkdownNode, append_element, append_node,
    append_text_source, finish_top_element, finish_top_element_source,
};

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
    source: Option<std::ops::Range<usize>>,
}

pub(super) fn append_html_fragment(
    stack: &mut Vec<ElementBuilder>,
    html: &str,
    source_start: usize,
) {
    for node in parse_html_fragment(html, source_start) {
        append_node(stack, node);
    }
}

pub(super) fn append_inline_html(stack: &mut Vec<ElementBuilder>, html: &str, source_start: usize) {
    match parse_single_html_tag(html, source_start) {
        Some(tag) => apply_html_tag(stack, tag),
        None => append_text_source(stack, html, Some(source_start..source_start + html.len())),
    }
}

fn parse_html_fragment(html: &str, source_start: usize) -> Vec<MarkdownNode> {
    let mut stack = vec![ElementBuilder::new("document", Vec::new())];
    let mut cursor = 0;

    while cursor < html.len() {
        let Some(relative_tag_start) = html[cursor..].find('<') else {
            append_text_source(
                &mut stack,
                &html[cursor..],
                Some(source_start + cursor..source_start + html.len()),
            );
            break;
        };

        let tag_start = cursor + relative_tag_start;
        if cursor < tag_start {
            append_text_source(
                &mut stack,
                &html[cursor..tag_start],
                Some(source_start + cursor..source_start + tag_start),
            );
        }

        match parse_html_tag_at(html, tag_start, source_start) {
            Some((tag, next)) => {
                apply_html_tag(&mut stack, tag);
                cursor = next;
            }
            None => {
                append_text_source(
                    &mut stack,
                    "<",
                    Some(source_start + tag_start..source_start + tag_start + 1),
                );
                cursor = tag_start + 1;
            }
        }
    }

    while stack.len() > 1 {
        finish_top_element(&mut stack);
    }

    stack
        .pop()
        .map(ElementBuilder::into_children)
        .unwrap_or_default()
}

fn parse_single_html_tag(html: &str, source_start: usize) -> Option<HtmlTag> {
    let trimmed = html.trim();
    let trim_start = html.find(trimmed).unwrap_or(0);
    let (tag, next) = parse_html_tag_at(trimmed, 0, source_start + trim_start)?;
    (next == trimmed.len()).then_some(tag)
}

fn parse_html_tag_at(html: &str, start: usize, source_start: usize) -> Option<(HtmlTag, usize)> {
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
                source: Some(source_start + start..source_start + end),
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
    parse_html_tag_content(content, Some(source_start + start..source_start + end + 1))
        .map(|tag| (tag, end + 1))
}

fn parse_html_tag_content(
    content: &str,
    source: Option<std::ops::Range<usize>>,
) -> Option<HtmlTag> {
    if content.is_empty()
        || content.starts_with('!')
        || content.starts_with('?')
        || content.starts_with("!--")
    {
        return Some(HtmlTag {
            kind: HtmlTagKind::SelfClosing,
            name: "comment".to_string(),
            attrs: Vec::new(),
            source,
        });
    }

    if let Some(rest) = content.strip_prefix('/') {
        let name = read_html_name(rest.trim_start())?;
        return Some(HtmlTag {
            kind: HtmlTagKind::End,
            name: name.to_ascii_lowercase(),
            attrs: Vec::new(),
            source,
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
        source,
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
        HtmlTagKind::Start => {
            stack.push(ElementBuilder::with_source(
                &tag.name, tag.attrs, tag.source,
            ));
        }
        HtmlTagKind::SelfClosing => append_element(
            stack,
            MarkdownElement::with_source(&tag.name, tag.attrs, Vec::new(), tag.source),
        ),
        HtmlTagKind::End => finish_matching_html_element(stack, &tag.name, tag.source),
    }
}

fn finish_matching_html_element(
    stack: &mut Vec<ElementBuilder>,
    name: &str,
    source: Option<std::ops::Range<usize>>,
) {
    let Some(index) = stack
        .iter()
        .rposition(|builder| builder.tag.eq_ignore_ascii_case(name))
    else {
        return;
    };

    if let Some(builder) = stack.get_mut(index) {
        builder.merge_source(source.clone());
    }

    while stack.len() > index {
        finish_top_element_source(stack, None);
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
