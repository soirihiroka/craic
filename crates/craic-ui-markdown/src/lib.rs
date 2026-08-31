#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use craic_language::{SyntaxHighlighter, destination_target, detected_links};
use gtk::glib::translate::IntoGlib;
use gtk::pango;
use gtk::prelude::*;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

pub use craic_language::LinkTarget;

pub type LinkHandler = Rc<dyn Fn(LinkTarget)>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderOptions {
    pub monospace: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveStyle {
    Heading,
    Quote,
    CodeBlock,
    Emphasis,
    Strong,
    Strikethrough,
    Link,
}

struct Renderer {
    buffer: gtk::TextBuffer,
    active_tags: Vec<(ActiveStyle, gtk::TextTag)>,
    links: HashMap<String, LinkTarget>,
    next_link_id: usize,
    code_language: Option<String>,
    lists: Vec<Option<u64>>,
    table_cell: usize,
}

static MARKDOWN_CSS_INSTALLED: OnceLock<()> = OnceLock::new();

pub fn render(markdown: &str, on_link: LinkHandler) -> gtk::Widget {
    render_with_options(markdown, RenderOptions::default(), on_link)
}

pub fn render_with_options(
    markdown: &str,
    options: RenderOptions,
    on_link: LinkHandler,
) -> gtk::Widget {
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .monospace(options.monospace)
        .left_margin(0)
        .right_margin(0)
        .top_margin(0)
        .bottom_margin(0)
        .build();
    view.add_css_class("craic-markdown");
    MARKDOWN_CSS_INSTALLED.get_or_init(|| {
        let background = gtk::CssProvider::new();
        background.load_from_data(
            "textview.craic-markdown, textview.craic-markdown text { background-color: transparent; }",
        );
        gtk::style_context_add_provider_for_display(
            &view.display(),
            &background,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
    view.set_hexpand(true);

    let buffer = view.buffer();
    let mut renderer = Renderer {
        buffer,
        active_tags: Vec::new(),
        links: HashMap::new(),
        next_link_id: 0,
        code_language: None,
        lists: Vec::new(),
        table_cell: 0,
    };
    renderer.render(markdown);
    let links = Rc::new(renderer.links);

    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released({
        let view = view.downgrade();
        let links = links.clone();
        let on_link = on_link.clone();
        move |_, _, x, y| {
            let Some(view) = view.upgrade() else {
                return;
            };
            if let Some(target) = link_at_point(&view, &links, x, y) {
                on_link(target);
            }
        }
    });
    view.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let view = view.downgrade();
        let links = links.clone();
        move |_, x, y| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let cursor = link_at_point(&view, &links, x, y)
                .is_some()
                .then_some("pointer");
            view.set_cursor_from_name(cursor);
        }
    });
    motion.connect_leave({
        let view = view.downgrade();
        move |_| {
            if let Some(view) = view.upgrade() {
                view.set_cursor_from_name(None);
            }
        }
    });
    view.add_controller(motion);

    view.upcast()
}

fn link_at_point(
    view: &gtk::TextView,
    links: &HashMap<String, LinkTarget>,
    x: f64,
    y: f64,
) -> Option<LinkTarget> {
    let (buffer_x, buffer_y) = view.window_to_buffer_coords(
        gtk::TextWindowType::Widget,
        x.round() as i32,
        y.round() as i32,
    );
    let iter = view.iter_at_location(buffer_x, buffer_y)?;
    iter.tags().into_iter().find_map(|tag| {
        let name = tag.name()?;
        links.get(name.as_str()).cloned()
    })
}

impl Renderer {
    fn render(&mut self, markdown: &str) {
        let options = Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_GFM;
        for event in Parser::new_ext(markdown, options) {
            self.event(event);
        }
        self.trim_trailing_newlines();
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.code_language.is_some() {
                    self.insert_code(&text);
                } else {
                    self.insert_text(&text);
                }
            }
            Event::Code(code) => {
                let tag = self.add_tag(
                    gtk::TextTag::builder()
                        .family("monospace")
                        .foreground("#c061cb")
                        .build(),
                );
                self.insert_detected(&code, &[tag]);
            }
            Event::InlineMath(math) => {
                let tag = self.add_tag(
                    gtk::TextTag::builder()
                        .family("monospace")
                        .style(pango::Style::Italic)
                        .build(),
                );
                self.insert_plain(&math, &[tag]);
            }
            Event::DisplayMath(math) => {
                self.ensure_block_start();
                let tag = self.add_tag(
                    gtk::TextTag::builder()
                        .family("monospace")
                        .style(pango::Style::Italic)
                        .left_margin(12)
                        .build(),
                );
                self.insert_plain(&math, &[tag]);
                self.finish_block();
            }
            Event::SoftBreak | Event::HardBreak => self.insert_plain("\n", &[]),
            Event::Rule => {
                self.ensure_block_start();
                self.insert_plain("────────────────────────", &[]);
                self.finish_block();
            }
            Event::TaskListMarker(checked) => {
                self.insert_plain(if checked { "☑ " } else { "☐ " }, &[]);
            }
            Event::Html(html) | Event::InlineHtml(html) => self.insert_text(&html),
            Event::FootnoteReference(reference) => {
                self.insert_plain("[", &[]);
                self.insert_text(&reference);
                self.insert_plain("]", &[]);
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.ensure_block_start(),
            Tag::Heading { level, .. } => {
                self.ensure_block_start();
                let scale = match level {
                    HeadingLevel::H1 => 1.5,
                    HeadingLevel::H2 => 1.35,
                    HeadingLevel::H3 => 1.2,
                    HeadingLevel::H4 => 1.1,
                    HeadingLevel::H5 | HeadingLevel::H6 => 1.0,
                };
                self.push_style(
                    ActiveStyle::Heading,
                    gtk::TextTag::builder()
                        .weight(pango::Weight::Bold.into_glib())
                        .scale(scale)
                        .pixels_above_lines(6)
                        .pixels_below_lines(3)
                        .build(),
                );
            }
            Tag::BlockQuote(_) => {
                self.ensure_block_start();
                self.push_style(
                    ActiveStyle::Quote,
                    gtk::TextTag::builder()
                        .style(pango::Style::Italic)
                        .foreground("#9a9996")
                        .left_margin(16)
                        .build(),
                );
            }
            Tag::CodeBlock(kind) => {
                self.ensure_block_start();
                self.code_language = Some(match kind {
                    CodeBlockKind::Indented => String::new(),
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                });
                self.push_style(
                    ActiveStyle::CodeBlock,
                    gtk::TextTag::builder()
                        .family("monospace")
                        .left_margin(12)
                        .pixels_above_lines(4)
                        .pixels_below_lines(4)
                        .build(),
                );
            }
            Tag::List(start) => {
                self.ensure_block_start();
                self.lists.push(start);
            }
            Tag::Item => {
                self.ensure_line_start();
                let depth = self.lists.len().saturating_sub(1);
                self.insert_plain(&"  ".repeat(depth), &[]);
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "• ".to_owned(),
                };
                self.insert_plain(&marker, &[]);
            }
            Tag::Emphasis => self.push_style(
                ActiveStyle::Emphasis,
                gtk::TextTag::builder().style(pango::Style::Italic).build(),
            ),
            Tag::Strong => self.push_style(
                ActiveStyle::Strong,
                gtk::TextTag::builder()
                    .weight(pango::Weight::Bold.into_glib())
                    .build(),
            ),
            Tag::Strikethrough => self.push_style(
                ActiveStyle::Strikethrough,
                gtk::TextTag::builder().strikethrough(true).build(),
            ),
            Tag::Link { dest_url, .. } => {
                let target = destination_target(&dest_url);
                self.push_link(target);
            }
            Tag::Image { dest_url, .. } => {
                self.insert_plain("🖼 ", &[]);
                let target = destination_target(&dest_url);
                self.push_link(target);
            }
            Tag::Table(_) => {
                self.ensure_block_start();
                self.table_cell = 0;
            }
            Tag::TableHead => {}
            Tag::TableRow => {
                self.ensure_line_start();
                self.table_cell = 0;
            }
            Tag::TableCell => {
                if self.table_cell > 0 {
                    self.insert_plain("\t", &[]);
                }
                self.table_cell += 1;
            }
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.finish_block(),
            TagEnd::Heading(_) => {
                self.pop_style(ActiveStyle::Heading);
                self.finish_block();
            }
            TagEnd::BlockQuote(_) => {
                self.pop_style(ActiveStyle::Quote);
                self.finish_block();
            }
            TagEnd::CodeBlock => {
                self.pop_style(ActiveStyle::CodeBlock);
                self.code_language = None;
                self.finish_block();
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.ensure_block_gap();
                }
            }
            TagEnd::Item | TagEnd::TableRow => self.ensure_line_start(),
            TagEnd::Emphasis => self.pop_style(ActiveStyle::Emphasis),
            TagEnd::Strong => self.pop_style(ActiveStyle::Strong),
            TagEnd::Strikethrough => self.pop_style(ActiveStyle::Strikethrough),
            TagEnd::Link | TagEnd::Image => self.pop_style(ActiveStyle::Link),
            TagEnd::Table => self.ensure_block_gap(),
            TagEnd::HtmlBlock
            | TagEnd::TableHead
            | TagEnd::TableCell
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn push_style(&mut self, style: ActiveStyle, tag: gtk::TextTag) {
        let tag = self.add_tag(tag);
        self.active_tags.push((style, tag));
    }

    fn pop_style(&mut self, style: ActiveStyle) {
        if let Some(index) = self
            .active_tags
            .iter()
            .rposition(|(active, _)| *active == style)
        {
            self.active_tags.remove(index);
        }
    }

    fn push_link(&mut self, target: LinkTarget) {
        let name = format!("markdown-link-{}", self.next_link_id);
        self.next_link_id += 1;
        let tag = self.add_tag(
            gtk::TextTag::builder()
                .name(&name)
                .foreground("#3584e4")
                .underline(pango::Underline::Single)
                .build(),
        );
        self.links.insert(name, target);
        self.active_tags.push((ActiveStyle::Link, tag));
    }

    fn insert_text(&mut self, text: &str) {
        if self
            .active_tags
            .iter()
            .any(|(style, _)| *style == ActiveStyle::Link)
        {
            self.insert_plain(text, &[]);
        } else {
            self.insert_detected(text, &[]);
        }
    }

    fn insert_detected(&mut self, text: &str, extra_tags: &[gtk::TextTag]) {
        let mut cursor = 0;
        for (start, end, target) in detected_links(text) {
            self.insert_plain(&text[cursor..start], extra_tags);
            let name = format!("markdown-link-{}", self.next_link_id);
            self.next_link_id += 1;
            let link_tag = self.add_tag(
                gtk::TextTag::builder()
                    .name(&name)
                    .foreground("#3584e4")
                    .underline(pango::Underline::Single)
                    .build(),
            );
            self.links.insert(name, target);
            let mut tags = extra_tags.to_vec();
            tags.push(link_tag);
            self.insert_plain(&text[start..end], &tags);
            cursor = end;
        }
        self.insert_plain(&text[cursor..], extra_tags);
    }

    fn insert_code(&mut self, code: &str) {
        let start = self.buffer.char_count();
        self.insert_detected(code, &[]);
        let Some(language) = self.code_language.as_deref().filter(|it| !it.is_empty()) else {
            return;
        };
        let mut highlighter = SyntaxHighlighter::new(language);
        highlighter.set_source(code);
        let mut colors = HashMap::<String, gtk::TextTag>::new();
        for range in highlighter.highlight_current() {
            if range.start >= range.end
                || range.end > code.len()
                || !code.is_char_boundary(range.start)
                || !code.is_char_boundary(range.end)
            {
                continue;
            }
            let color = syntax_color(range.style.color());
            let tag = colors
                .entry(color.clone())
                .or_insert_with(|| {
                    self.add_tag(
                        gtk::TextTag::builder()
                            .foreground(&color)
                            .family("monospace")
                            .build(),
                    )
                })
                .clone();
            let range_start = start + code[..range.start].chars().count() as i32;
            let range_end = start + code[..range.end].chars().count() as i32;
            self.buffer.apply_tag(
                &tag,
                &self.buffer.iter_at_offset(range_start),
                &self.buffer.iter_at_offset(range_end),
            );
        }
    }

    fn insert_plain(&mut self, text: &str, extra_tags: &[gtk::TextTag]) {
        if text.is_empty() {
            return;
        }
        let mut tags: Vec<&gtk::TextTag> = self.active_tags.iter().map(|(_, tag)| tag).collect();
        tags.extend(extra_tags);
        self.buffer
            .insert_with_tags(&mut self.buffer.end_iter(), text, &tags);
    }

    fn add_tag(&self, tag: gtk::TextTag) -> gtk::TextTag {
        self.buffer.tag_table().add(&tag);
        tag
    }

    fn ensure_block_start(&mut self) {
        if self.buffer.char_count() > 0 && !self.ends_with("\n") {
            self.insert_plain("\n", &[]);
        }
    }

    fn finish_block(&mut self) {
        self.ensure_block_gap();
    }

    fn ensure_line_start(&mut self) {
        if self.buffer.char_count() > 0 && !self.ends_with("\n") {
            self.insert_plain("\n", &[]);
        }
    }

    fn ensure_block_gap(&mut self) {
        if self.buffer.char_count() == 0 {
            return;
        }
        if !self.ends_with("\n") {
            self.insert_plain("\n\n", &[]);
        } else if !self.ends_with("\n\n") {
            self.insert_plain("\n", &[]);
        }
    }

    fn ends_with(&self, suffix: &str) -> bool {
        let chars = suffix.chars().count() as i32;
        let end = self.buffer.end_iter();
        if end.offset() < chars {
            return false;
        }
        self.buffer
            .text(
                &self.buffer.iter_at_offset(end.offset() - chars),
                &end,
                true,
            )
            .as_str()
            == suffix
    }

    fn trim_trailing_newlines(&self) {
        let end = self.buffer.end_iter();
        let text = self.buffer.text(&self.buffer.start_iter(), &end, true);
        let trimmed = text.trim_end_matches('\n');
        let remove = text[trimmed.len()..].chars().count() as i32;
        if remove == 0 {
            return;
        }
        self.buffer.delete(
            &mut self.buffer.iter_at_offset(end.offset() - remove),
            &mut self.buffer.end_iter(),
        );
    }
}

fn syntax_color((red, green, blue): (f64, f64, f64)) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (red * 255.0).round() as u8,
        (green * 255.0).round() as u8,
        (blue * 255.0).round() as u8
    )
}
