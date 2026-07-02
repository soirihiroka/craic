use super::{PreviewMatchRequest, PreviewRequest};
use crate::git;
use crate::language_support::SyntaxHighlighter;
use crate::ui::components::markdown_preview::MarkdownPreviewDocument;
use gtk::gio;
use gtk::prelude::*;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use webkit6::prelude::*;

const MARKDOWN_SCROLL_MESSAGE_HANDLER: &str = "craicMarkdownScroll";
const MARKDOWN_SCROLL_SYNC_SCRIPT: &str = r#"
(() => {
  if (window.__craicMarkdownScrollSyncInstalled) return;
  window.__craicMarkdownScrollSyncInstalled = true;
  window.__craicMarkdownSuppressScroll = false;
  const MAX_OVERSHOOT_DISTANCE = 100;
  const OVERSHOOT_DECAY = 0.80;
  let topOvershoot = 0;
  let bottomOvershoot = 0;
  let overshootFrame = 0;
  let overshootTopLayer = null;
  let overshootBottomLayer = null;
  let touchY = null;

  const anchors = () => Array.from(document.querySelectorAll("[data-source-start]"))
    .map((element) => ({
      element,
      source: Number.parseInt(element.getAttribute("data-source-start") || "0", 10),
      y: element.getBoundingClientRect().top + window.scrollY,
    }))
    .filter((anchor) => Number.isFinite(anchor.source) && Number.isFinite(anchor.y))
    .sort((a, b) => a.y - b.y || a.source - b.source);

  const documentBottom = () => Math.max(
    document.documentElement ? document.documentElement.scrollHeight : 0,
    document.body ? document.body.scrollHeight : 0,
    window.innerHeight
  );

  const maxScrollY = () => Math.max(0, documentBottom() - window.innerHeight);

  const ensureOvershootLayers = () => {
    if (overshootTopLayer && overshootBottomLayer) return;

    overshootTopLayer = document.createElement("div");
    overshootTopLayer.className = "craic-overshoot craic-overshoot-top";
    overshootBottomLayer = document.createElement("div");
    overshootBottomLayer.className = "craic-overshoot craic-overshoot-bottom";
    document.documentElement.append(overshootTopLayer, overshootBottomLayer);
  };

  const updateOvershootLayer = (layer, distance) => {
    const clamped = Math.min(MAX_OVERSHOOT_DISTANCE, Math.max(0, distance));
    const strength = Math.sqrt(clamped / MAX_OVERSHOOT_DISTANCE);
    layer.style.height = `${clamped}px`;
    layer.style.opacity = String(strength);
  };

  const updateOvershoot = () => {
    ensureOvershootLayers();
    updateOvershootLayer(overshootTopLayer, topOvershoot);
    updateOvershootLayer(overshootBottomLayer, bottomOvershoot);
  };

  const scheduleOvershootDecay = () => {
    if (overshootFrame) return;

    const tick = () => {
      topOvershoot = topOvershoot > 0.5 ? topOvershoot * OVERSHOOT_DECAY : 0;
      bottomOvershoot = bottomOvershoot > 0.5 ? bottomOvershoot * OVERSHOOT_DECAY : 0;
      updateOvershoot();

      if (topOvershoot > 0 || bottomOvershoot > 0) {
        overshootFrame = requestAnimationFrame(tick);
      } else {
        overshootFrame = 0;
      }
    };

    overshootFrame = requestAnimationFrame(tick);
  };

  const pullOvershoot = (edge, overflow) => {
    if (!Number.isFinite(overflow) || overflow <= 0) return;

    const impulse = Math.min(42, Math.max(2, overflow * 0.72));
    if (edge === "top") {
      bottomOvershoot = 0;
      topOvershoot = Math.min(MAX_OVERSHOOT_DISTANCE, topOvershoot + impulse);
    } else {
      topOvershoot = 0;
      bottomOvershoot = Math.min(MAX_OVERSHOOT_DISTANCE, bottomOvershoot + impulse);
    }

    updateOvershoot();
    scheduleOvershootDecay();
  };

  const pullOvershootForDelta = (deltaY) => {
    if (!Number.isFinite(deltaY) || Math.abs(deltaY) <= Number.EPSILON) return;

    const maxY = maxScrollY();
    const desired = window.scrollY + deltaY;
    if (maxY <= 0) {
      pullOvershoot(deltaY < 0 ? "top" : "bottom", Math.abs(deltaY));
    } else if (desired < 0) {
      pullOvershoot("top", -desired);
    } else if (desired > maxY) {
      pullOvershoot("bottom", desired - maxY);
    }
  };

  const wheelDeltaY = (event) => {
    if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) return event.deltaY * 16;
    if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) return event.deltaY * window.innerHeight;
    return event.deltaY;
  };

  const sourceAtViewportTop = () => {
    const points = anchors();
    if (points.length === 0) return 0;

    const y = window.scrollY;
    if (y <= points[0].y) return points[0].source;

    for (let i = 0; i < points.length - 1; i++) {
      const current = points[i];
      const next = points[i + 1];
      if (y > next.y) continue;

      const domSpan = Math.max(1, next.y - current.y);
      const progress = Math.min(1, Math.max(0, (y - current.y) / domSpan));
      return Math.round(current.source + (next.source - current.source) * progress);
    }

    const last = points[points.length - 1];
    return last.source;
  };

  const yForSource = (source) => {
    const points = anchors();
    if (points.length === 0) return 0;

    const target = Number.isFinite(source) ? source : 0;
    if (target <= points[0].source) return points[0].y;

    for (let i = 0; i < points.length - 1; i++) {
      const current = points[i];
      const next = points[i + 1];
      if (target > next.source) continue;

      const sourceSpan = Math.max(1, next.source - current.source);
      const progress = Math.min(1, Math.max(0, (target - current.source) / sourceSpan));
      return current.y + (next.y - current.y) * progress;
    }

    return points[points.length - 1].y;
  };

  const post = () => {
    if (window.__craicMarkdownSuppressScroll) return;
    try {
      window.webkit.messageHandlers.craicMarkdownScroll.postMessage(sourceAtViewportTop());
    } catch (_) {}
  };

  let scheduled = false;
  const schedulePost = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      post();
    });
  };

  window.__craicMarkdownSetSourceOffset = (value) => {
    const numeric = Number(value);
    const y = yForSource(numeric);
    const maxY = Math.max(0, documentBottom() - window.innerHeight);
    window.__craicMarkdownSuppressScroll = true;
    window.scrollTo(window.scrollX, Math.min(maxY, Math.max(0, y)));
    requestAnimationFrame(() => {
      window.__craicMarkdownSuppressScroll = false;
    });
  };

  window.addEventListener("scroll", schedulePost, { passive: true });
  window.addEventListener("wheel", (event) => pullOvershootForDelta(wheelDeltaY(event)), { passive: true });
  window.addEventListener("touchstart", (event) => {
    touchY = event.touches.length > 0 ? event.touches[0].clientY : null;
  }, { passive: true });
  window.addEventListener("touchmove", (event) => {
    if (touchY === null || event.touches.length === 0) return;
    const nextY = event.touches[0].clientY;
    pullOvershootForDelta(touchY - nextY);
    touchY = nextY;
  }, { passive: true });
  window.addEventListener("touchend", () => { touchY = null; }, { passive: true });
  window.addEventListener("touchcancel", () => { touchY = null; }, { passive: true });
  window.addEventListener("resize", schedulePost);
  window.addEventListener("load", schedulePost);
  ensureOvershootLayers();
  schedulePost();
})();
"#;

type SourceOffsetCallback = Rc<dyn Fn(usize)>;

struct MarkdownPreviewLoad {
    text: String,
    document: MarkdownPreviewDocument,
    comparison: Option<git::FileComparison>,
    markdown_lint_issues: Vec<crate::markdown_lint::MarkdownLintIssue>,
    spellcheck_issues: Vec<crate::spellcheck::SpellcheckIssue>,
}

pub(in crate::ui::pages::file) struct MarkdownPreview {
    pub(in crate::ui::pages::file) root: webkit6::WebView,
    signature: RefCell<Option<super::ContentSignature>>,
    base_uri: Rc<RefCell<Option<String>>>,
    allow_preview_navigation: Rc<Cell<bool>>,
    source_offset_callbacks: Rc<RefCell<Vec<SourceOffsetCallback>>>,
    target_source_offset: Rc<Cell<Option<usize>>>,
}

impl MarkdownPreview {
    pub(in crate::ui::pages::file) fn new() -> Rc<Self> {
        let user_content_manager = webkit6::UserContentManager::new();
        let base_uri = Rc::new(RefCell::new(None));
        let allow_preview_navigation = Rc::new(Cell::new(false));
        let source_offset_callbacks = Rc::new(RefCell::new(Vec::<SourceOffsetCallback>::new()));
        let target_source_offset = Rc::new(Cell::new(None::<usize>));

        install_scroll_bridge(
            &user_content_manager,
            Rc::clone(&source_offset_callbacks),
            Rc::clone(&target_source_offset),
        );

        let root = webkit6::WebView::builder()
            .user_content_manager(&user_content_manager)
            .build();

        root.set_hexpand(true);
        root.set_vexpand(true);
        root.add_css_class("markdown-preview");
        install_navigation_policy(
            &root,
            Rc::clone(&base_uri),
            Rc::clone(&allow_preview_navigation),
        );
        install_scroll_target_reapply(&root, Rc::clone(&target_source_offset));

        Rc::new(Self {
            root,
            signature: RefCell::new(None),
            base_uri,
            allow_preview_navigation,
            source_offset_callbacks,
            target_source_offset,
        })
    }

    pub(in crate::ui::pages::file) fn set_markdown_html(
        &self,
        html: &str,
        signature: super::ContentSignature,
        path: Option<&Path>,
    ) {
        let next_base_uri = path.and_then(|path| {
            let mut uri = path.parent().map(gio::File::for_path)?.uri().to_string();
            if !uri.ends_with('/') {
                uri.push('/');
            }
            Some(uri)
        });

        if self.signature.borrow().as_ref() == Some(&signature)
            && self.base_uri.borrow().as_ref() == next_base_uri.as_ref()
        {
            return;
        }

        self.signature.replace(Some(signature));
        self.base_uri.replace(next_base_uri.clone());
        self.allow_preview_navigation.set(true);
        self.root.load_html(html, next_base_uri.as_deref());
    }

    pub(in crate::ui::pages::file) fn connect_source_offset_changed<F>(&self, callback: F)
    where
        F: Fn(usize) + 'static,
    {
        self.source_offset_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub(in crate::ui::pages::file) fn set_source_offset(&self, offset: usize) {
        self.target_source_offset.set(Some(offset));
        set_web_view_source_offset(&self.root, offset);
    }
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

fn install_scroll_bridge(
    user_content_manager: &webkit6::UserContentManager,
    source_offset_callbacks: Rc<RefCell<Vec<SourceOffsetCallback>>>,
    target_source_offset: Rc<Cell<Option<usize>>>,
) {
    if !user_content_manager.register_script_message_handler(MARKDOWN_SCROLL_MESSAGE_HANDLER, None)
    {
        log::warn!("failed to register markdown preview scroll message handler");
    }

    user_content_manager.add_script(&webkit6::UserScript::new(
        MARKDOWN_SCROLL_SYNC_SCRIPT,
        webkit6::UserContentInjectedFrames::TopFrame,
        webkit6::UserScriptInjectionTime::End,
        &[],
        &[],
    ));

    user_content_manager.connect_script_message_received(
        Some(MARKDOWN_SCROLL_MESSAGE_HANDLER),
        move |_, value| {
            let offset = normalize_source_offset(value.to_double());
            target_source_offset.set(Some(offset));

            for callback in source_offset_callbacks.borrow().iter() {
                callback(offset);
            }
        },
    );
}

fn install_scroll_target_reapply(
    web_view: &webkit6::WebView,
    target_source_offset: Rc<Cell<Option<usize>>>,
) {
    web_view.connect_load_changed(move |web_view, event| {
        if matches!(event, webkit6::LoadEvent::Finished) {
            if let Some(offset) = target_source_offset.get() {
                set_web_view_source_offset(web_view, offset);
            }
        }
    });
}

fn set_web_view_source_offset(web_view: &webkit6::WebView, offset: usize) {
    let script = format!(
        "if (window.__craicMarkdownSetSourceOffset) window.__craicMarkdownSetSourceOffset({offset});"
    );

    web_view.evaluate_javascript(&script, None, None, None::<&gio::Cancellable>, |_| {});
}

fn normalize_source_offset(offset: f64) -> usize {
    if offset.is_finite() && offset > 0.0 {
        offset.round() as usize
    } else {
        0
    }
}

fn install_navigation_policy(
    web_view: &webkit6::WebView,
    base_uri: Rc<RefCell<Option<String>>>,
    allow_preview_navigation: Rc<Cell<bool>>,
) {
    web_view.connect_load_changed({
        let allow_preview_navigation = Rc::clone(&allow_preview_navigation);
        move |_, event| {
            if matches!(event, webkit6::LoadEvent::Finished) {
                allow_preview_navigation.set(false);
            }
        }
    });

    web_view.connect_load_failed({
        let allow_preview_navigation = Rc::clone(&allow_preview_navigation);
        move |_, _, _, _| {
            allow_preview_navigation.set(false);
            false
        }
    });

    web_view.connect_decide_policy(move |_, decision, decision_type| {
        let (is_navigation_decision, is_new_window_action) = match decision_type {
            webkit6::PolicyDecisionType::NavigationAction => (true, false),
            webkit6::PolicyDecisionType::NewWindowAction => (true, true),
            _ => (false, false),
        };

        if !is_navigation_decision {
            return false;
        }

        let current_base_uri = base_uri.borrow().clone();
        let Some(navigation_decision) =
            decision.downcast_ref::<webkit6::NavigationPolicyDecision>()
        else {
            if allow_preview_navigation.replace(false) {
                return false;
            }

            decision.ignore();
            return true;
        };

        let action = navigation_decision.navigation_action();
        let uri = navigation_uri(action.as_ref());

        if allow_preview_navigation.replace(false)
            && !is_user_requested_navigation(action.as_ref())
            && is_preview_document_uri(uri.as_deref(), current_base_uri.as_deref())
        {
            return false;
        }

        decision.ignore();

        if should_launch_external(is_new_window_action, action.as_ref()) {
            if let Some(uri) = uri.as_deref() {
                open_external_uri(uri, current_base_uri.as_deref());
            }
        }

        true
    });
}

fn navigation_uri(action: Option<&webkit6::NavigationAction>) -> Option<String> {
    action?.request()?.uri().map(|uri| uri.as_str().to_string())
}

fn is_user_requested_navigation(action: Option<&webkit6::NavigationAction>) -> bool {
    let Some(action) = action else {
        return false;
    };

    action.is_user_gesture()
        || matches!(
            action.navigation_type(),
            webkit6::NavigationType::LinkClicked
        )
}

fn should_launch_external(
    is_new_window_action: bool,
    action: Option<&webkit6::NavigationAction>,
) -> bool {
    is_new_window_action || is_user_requested_navigation(action)
}

fn open_external_uri(uri: &str, base_uri: Option<&str>) {
    if !should_open_uri_externally(uri, base_uri) {
        return;
    }

    if let Err(err) = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>) {
        log::warn!("failed to open markdown preview URI externally: {err}");
    }
}

fn should_open_uri_externally(uri: &str, base_uri: Option<&str>) -> bool {
    if uri.is_empty() || is_same_preview_document_uri(uri, base_uri) {
        return false;
    }

    let scheme = uri
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .unwrap_or_default();

    !["about", "data", "javascript"]
        .iter()
        .any(|blocked| scheme.eq_ignore_ascii_case(blocked))
}

fn is_preview_document_uri(uri: Option<&str>, base_uri: Option<&str>) -> bool {
    let Some(uri) = uri else {
        return true;
    };

    uri == "about:blank" || is_same_preview_document_uri(uri, base_uri)
}

fn is_same_preview_document_uri(uri: &str, base_uri: Option<&str>) -> bool {
    let Some(base_uri) = base_uri else {
        return false;
    };

    strip_fragment(uri) == strip_fragment(base_uri)
}

fn strip_fragment(uri: &str) -> &str {
    uri.split_once('#').map(|(uri, _)| uri).unwrap_or(uri)
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

pub(in crate::ui::pages::file) fn markdown_to_html(markdown: &str) -> String {
    let body = markdown_fragment_to_html(markdown);

    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>
:root {{
  color-scheme: light dark;
  font-family: Cantarell, sans-serif;
  font-size: 15px;
  line-height: 1.5;
}}
html {{
  scroll-behavior: auto;
}}
body {{
  margin: 0;
  padding: 24px;
  background: transparent;
  color: CanvasText;
}}
pre {{
  overflow-x: auto;
  padding: 12px;
  border-radius: 8px;
  background: color-mix(in srgb, CanvasText 9%, transparent);
}}
code {{
  font-family: monospace;
}}
img {{
  max-width: 100%;
  height: auto;
  object-fit: contain;
}}
.source-anchor {{
  display: block;
  height: 0;
  overflow: hidden;
}}
.craic-overshoot {{
  position: fixed;
  left: 0;
  right: 0;
  height: 0;
  opacity: 0;
  pointer-events: none;
  z-index: 2147483647;
  background-color: transparent;
  background-repeat: no-repeat;
  border: none;
  box-shadow: none;
  transition: opacity 80ms ease-out;
}}
.craic-overshoot-top {{
  top: 0;
  background-image:
    radial-gradient(farthest-side at top,
      color-mix(in srgb, CanvasText 12%, transparent) 85%,
      transparent),
    radial-gradient(farthest-side at top,
      color-mix(in srgb, CanvasText 5%, transparent),
      transparent);
  background-size: 100% 3%, 100% 50%;
  background-position: top;
}}
.craic-overshoot-bottom {{
  bottom: 0;
  background-image:
    radial-gradient(farthest-side at bottom,
      color-mix(in srgb, CanvasText 12%, transparent) 85%,
      transparent),
    radial-gradient(farthest-side at bottom,
      color-mix(in srgb, CanvasText 5%, transparent),
      transparent);
  background-size: 100% 3%, 100% 50%;
  background-position: bottom;
}}
a {{
  color: LinkText;
}}
</style>
<script>
(() => {{
  const preserveImageAspectRatios = () => {{
    for (const image of document.querySelectorAll("img[height]:not([width])")) {{
      const height = Number.parseFloat(image.getAttribute("height") || "");
      if (!Number.isFinite(height) || height <= 0) continue;
      image.style.maxHeight = `${{height}}px`;
      image.style.height = "auto";
      image.style.width = "auto";
    }}
  }};

  window.addEventListener("DOMContentLoaded", preserveImageAspectRatios);
  window.addEventListener("load", () => {{
    preserveImageAspectRatios();
  }});
}})();
</script>
</head>
<body><main class="markdown-body">
{body}
</main></body>
</html>"#
    )
}

pub(super) fn markdown_fragment_to_html(markdown: &str) -> String {
    if markdown.is_empty() {
        String::new()
    } else {
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
                        Event::Text(text) => {
                            code.push_str(text);
                        }
                        Event::Code(text) => {
                            code.push_str(text);
                        }
                        Event::SoftBreak | Event::HardBreak => {
                            code.push('\n');
                        }
                        Event::Html(html) => {
                            code.push_str(html);
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
}

pub(super) fn render_code_block(language: &str, code: &str) -> String {
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
