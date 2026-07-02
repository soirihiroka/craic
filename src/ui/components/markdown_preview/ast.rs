use std::ops::Range;

pub(super) type SourceRange = Range<usize>;

#[derive(Clone, Debug)]
pub(super) enum MarkdownNode {
    Text {
        text: String,
        source: Option<SourceRange>,
    },
    Element(MarkdownElement),
}

#[derive(Clone, Debug)]
pub(super) struct MarkdownElement {
    pub(super) tag: String,
    pub(super) attrs: Vec<HtmlAttribute>,
    pub(super) children: Vec<MarkdownNode>,
    pub(super) source: Option<SourceRange>,
}

#[derive(Clone, Debug)]
pub(super) struct HtmlAttribute {
    pub(super) name: String,
    pub(super) value: String,
}

#[derive(Clone, Debug)]
pub(super) struct ElementBuilder {
    pub(super) tag: String,
    attrs: Vec<HtmlAttribute>,
    children: Vec<MarkdownNode>,
    source: Option<SourceRange>,
}

impl ElementBuilder {
    pub(super) fn new(tag: &str, attrs: Vec<HtmlAttribute>) -> Self {
        Self::with_source(tag, attrs, None)
    }

    pub(super) fn with_source(
        tag: &str,
        attrs: Vec<HtmlAttribute>,
        source: Option<SourceRange>,
    ) -> Self {
        Self {
            tag: tag.to_ascii_lowercase(),
            attrs,
            children: Vec::new(),
            source,
        }
    }

    pub(super) fn merge_source(&mut self, source: Option<SourceRange>) {
        merge_source_range(&mut self.source, source);
    }

    pub(super) fn into_children(self) -> Vec<MarkdownNode> {
        self.children
    }
}

impl MarkdownElement {
    pub(super) fn new(tag: &str, attrs: Vec<HtmlAttribute>, children: Vec<MarkdownNode>) -> Self {
        Self::with_source(tag, attrs, children, None)
    }

    pub(super) fn with_source(
        tag: &str,
        attrs: Vec<HtmlAttribute>,
        children: Vec<MarkdownNode>,
        source: Option<SourceRange>,
    ) -> Self {
        Self {
            tag: tag.to_ascii_lowercase(),
            attrs,
            children,
            source,
        }
    }

    pub(super) fn empty(tag: &str) -> Self {
        Self::new(tag, Vec::new(), Vec::new())
    }

    pub(super) fn empty_with_source(tag: &str, source: Option<SourceRange>) -> Self {
        Self::with_source(tag, Vec::new(), Vec::new(), source)
    }
}

impl HtmlAttribute {
    pub(super) fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_ascii_lowercase(),
            value: value.to_string(),
        }
    }
}

pub(super) fn append_text(stack: &mut [ElementBuilder], text: &str) {
    append_text_source(stack, text, None);
}

pub(super) fn append_text_source(
    stack: &mut [ElementBuilder],
    text: &str,
    source: Option<SourceRange>,
) {
    if text.is_empty() {
        return;
    }

    if let Some(MarkdownNode::Text {
        text: previous,
        source: previous_source,
    }) = stack.last_mut().and_then(|top| top.children.last_mut())
    {
        previous.push_str(text);
        merge_source_range(previous_source, source.clone());
    } else if let Some(top) = stack.last_mut() {
        top.children.push(MarkdownNode::Text {
            text: text.to_string(),
            source: source.clone(),
        });
    }
    if let Some(top) = stack.last_mut() {
        top.merge_source(source);
    }
}

pub(super) fn append_element_with_text(
    stack: &mut Vec<ElementBuilder>,
    tag: &str,
    attrs: Vec<HtmlAttribute>,
    text: &str,
    source: Option<SourceRange>,
) {
    append_element(
        stack,
        MarkdownElement::with_source(
            tag,
            attrs,
            vec![MarkdownNode::Text {
                text: text.to_string(),
                source: source.clone(),
            }],
            source,
        ),
    );
}

pub(super) fn append_node(stack: &mut [ElementBuilder], node: MarkdownNode) {
    if let Some(top) = stack.last_mut() {
        top.merge_source(node_source(&node));
        top.children.push(node);
    }
}

pub(super) fn append_element(stack: &mut [ElementBuilder], element: MarkdownElement) {
    append_node(stack, MarkdownNode::Element(element));
}

pub(super) fn finish_top_element(stack: &mut Vec<ElementBuilder>) {
    finish_top_element_source(stack, None);
}

pub(super) fn finish_top_element_source(
    stack: &mut Vec<ElementBuilder>,
    source: Option<SourceRange>,
) {
    if stack.len() <= 1 {
        return;
    }

    let mut builder = stack.pop().expect("markdown element stack is not empty");
    builder.merge_source(source);
    if builder.tag == "img" && attr_value(&builder.attrs, "alt").is_none() {
        let alt = text_content(&builder.children);
        if !alt.is_empty() {
            builder.attrs.push(HtmlAttribute::new("alt", &alt));
        }
        builder.children.clear();
    }

    append_element(
        stack,
        MarkdownElement::with_source(
            &builder.tag,
            builder.attrs,
            builder.children,
            builder.source,
        ),
    );
}

pub(super) fn current_tag(stack: &[ElementBuilder]) -> Option<&str> {
    stack.last().map(|top| top.tag.as_str())
}

pub(super) fn attr_value(attrs: &[HtmlAttribute], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|attr| attr.name.eq_ignore_ascii_case(name))
        .map(|attr| attr.value.clone())
}

pub(super) fn text_content(nodes: &[MarkdownNode]) -> String {
    let mut text = String::new();
    for node in nodes {
        match node {
            MarkdownNode::Text { text: value, .. } => text.push_str(value),
            MarkdownNode::Element(element) => text.push_str(&text_content(&element.children)),
        }
    }
    text
}

pub(super) fn node_source(node: &MarkdownNode) -> Option<SourceRange> {
    match node {
        MarkdownNode::Text { source, .. } => source.clone(),
        MarkdownNode::Element(element) => element.source.clone(),
    }
}

pub(super) fn merge_source_range(
    existing: &mut Option<SourceRange>,
    incoming: Option<SourceRange>,
) {
    let Some(incoming) = incoming else {
        return;
    };

    match existing {
        Some(existing) => {
            existing.start = existing.start.min(incoming.start);
            existing.end = existing.end.max(incoming.end);
        }
        None => *existing = Some(incoming),
    }
}
