use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use webkit6::prelude::*;

type SourceOffsetCallback = Box<dyn Fn()>;

const SOURCE_OFFSET_MESSAGE_HANDLER: &str = "sourceOffsetChanged";
const OPEN_LINK_MESSAGE_HANDLER: &str = "openMarkdownLink";

pub struct MarkdownPreview {
    pub root: webkit6::WebView,
    loaded: Cell<bool>,
    has_document: Cell<bool>,
    pending_source_offset: Cell<Option<usize>>,
    viewport_source_offset: Cell<Option<usize>>,
    source_offset_changed_callbacks: RefCell<Vec<SourceOffsetCallback>>,
}

pub struct MarkdownPreviewDocument {
    html: String,
}

impl MarkdownPreviewDocument {
    pub fn parse(markdown: &str) -> Self {
        let html = crate::ui::markdown_preview_web::markdown_document_html(markdown);
        log::debug!(
            "markdown preview document rendered markdown_bytes={} html_bytes={}",
            markdown.len(),
            html.len(),
        );
        Self { html }
    }
}

impl MarkdownPreview {
    pub fn new() -> Rc<Self> {
        let user_content_manager = webkit6::UserContentManager::new();
        if !user_content_manager
            .register_script_message_handler(SOURCE_OFFSET_MESSAGE_HANDLER, None)
        {
            log::warn!("failed to register markdown preview source-offset message handler");
        }
        if !user_content_manager.register_script_message_handler(OPEN_LINK_MESSAGE_HANDLER, None) {
            log::warn!("failed to register markdown preview link message handler");
        }
        user_content_manager.add_script(&webkit6::UserScript::new(
            crate::ui::markdown_preview_web::SOURCE_MAP_SCRIPT,
            webkit6::UserContentInjectedFrames::TopFrame,
            webkit6::UserScriptInjectionTime::End,
            &[],
            &[],
        ));

        let root = webkit6::WebView::builder()
            .user_content_manager(&user_content_manager)
            .build();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_focusable(true);
        root.set_size_request(0, -1);
        root.add_css_class("markdown-preview");

        if let Some(settings) = webkit6::prelude::WebViewExt::settings(&root) {
            settings.set_enable_javascript_markup(false);
            settings.set_javascript_can_access_clipboard(false);
            settings.set_javascript_can_open_windows_automatically(false);
        }

        let preview = Rc::new(Self {
            root,
            loaded: Cell::new(false),
            has_document: Cell::new(false),
            pending_source_offset: Cell::new(None),
            viewport_source_offset: Cell::new(None),
            source_offset_changed_callbacks: RefCell::new(Vec::new()),
        });

        install_source_offset_handler(&preview, &user_content_manager);
        install_link_handler(&preview, &user_content_manager);
        install_navigation_policy(&preview.root);
        install_load_handlers(&preview);
        preview
    }

    pub fn set_document_with_base_path(
        &self,
        document: MarkdownPreviewDocument,
        base_path: Option<&Path>,
    ) {
        self.loaded.set(false);
        self.has_document.set(true);
        self.viewport_source_offset.set(None);
        self.root
            .load_html(&document.html, base_uri_for_path(base_path).as_deref());
    }

    pub fn source_offset_at_viewport_top(&self) -> Option<usize> {
        self.viewport_source_offset.get()
    }

    pub fn scroll_to_source_offset(&self, source_offset: usize) -> bool {
        self.pending_source_offset.set(Some(source_offset));
        if !self.loaded.get() {
            return false;
        }

        self.evaluate_scroll_to_source_offset(source_offset);
        true
    }

    pub fn connect_source_offset_changed<F: Fn() + 'static>(&self, callback: F) {
        self.source_offset_changed_callbacks
            .borrow_mut()
            .push(Box::new(callback));
    }

    fn update_viewport_source_offset(&self, offset: usize) {
        if self.viewport_source_offset.replace(Some(offset)) == Some(offset) {
            return;
        }

        for callback in self.source_offset_changed_callbacks.borrow().iter() {
            callback();
        }
    }

    fn finish_load(&self) {
        self.loaded.set(true);
        if let Some(source_offset) = self.pending_source_offset.take() {
            self.evaluate_scroll_to_source_offset(source_offset);
        } else {
            self.evaluate_report_source_offset();
        }
    }

    fn evaluate_report_source_offset(&self) {
        self.evaluate_javascript("window.CraicMarkdownPreview?.reportSourceOffset?.();");
    }

    fn evaluate_scroll_to_source_offset(&self, source_offset: usize) {
        self.evaluate_javascript(&format!(
            "window.CraicMarkdownPreview?.scrollToSourceOffset?.({source_offset});"
        ));
    }

    fn evaluate_javascript(&self, script: &str) {
        if !self.has_document.get() {
            return;
        }

        self.root.evaluate_javascript(
            script,
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            |result| {
                if let Err(err) = result {
                    log::debug!("markdown preview javascript evaluation failed: {err}");
                }
            },
        );
    }
}

fn install_source_offset_handler(
    preview: &Rc<MarkdownPreview>,
    user_content_manager: &webkit6::UserContentManager,
) {
    let preview = Rc::downgrade(preview);
    user_content_manager.connect_script_message_received(
        Some(SOURCE_OFFSET_MESSAGE_HANDLER),
        move |_, value| {
            let Some(preview) = preview.upgrade() else {
                return;
            };
            if !value.is_number() {
                return;
            }

            let offset = value.to_double();
            if offset.is_finite() {
                preview.update_viewport_source_offset(offset.max(0.0).round() as usize);
            }
        },
    );
}

fn install_link_handler(
    preview: &Rc<MarkdownPreview>,
    user_content_manager: &webkit6::UserContentManager,
) {
    let preview = Rc::downgrade(preview);
    user_content_manager.connect_script_message_received(
        Some(OPEN_LINK_MESSAGE_HANDLER),
        move |_, value| {
            let Some(preview) = preview.upgrade() else {
                return;
            };
            let uri = value.to_str().to_string();
            open_markdown_link(preview.root.upcast_ref(), &uri);
        },
    );
}

fn install_navigation_policy(web_view: &webkit6::WebView) {
    web_view.connect_decide_policy(|web_view, decision, decision_type| {
        if !matches!(decision_type, webkit6::PolicyDecisionType::NavigationAction) {
            return false;
        }

        let Some(action) = decision
            .downcast_ref::<webkit6::NavigationPolicyDecision>()
            .and_then(|navigation_decision| navigation_decision.navigation_action())
        else {
            return false;
        };

        if !action.is_user_gesture()
            || !matches!(
                action.navigation_type(),
                webkit6::NavigationType::LinkClicked
            )
        {
            return false;
        }

        let uri = action
            .request()
            .and_then(|request| request.uri())
            .map(|uri| uri.to_string())
            .unwrap_or_default();

        if uri.is_empty() || uri == "about:blank" {
            return false;
        }

        decision.ignore();
        open_markdown_link(web_view.upcast_ref(), &uri);
        true
    });
}

fn install_load_handlers(preview: &Rc<MarkdownPreview>) {
    preview.root.connect_load_changed({
        let preview = Rc::downgrade(preview);
        move |web_view, event| {
            let uri = web_view
                .uri()
                .map(|uri| uri.to_string())
                .unwrap_or_default();
            log::debug!("markdown preview webview load event={event:?} uri={uri}");

            if !matches!(event, webkit6::LoadEvent::Finished) {
                return;
            }

            let Some(preview) = preview.upgrade() else {
                return;
            };
            preview.finish_load();
        }
    });

    preview.root.connect_load_failed(|_, event, uri, err| {
        log::warn!("markdown preview webview load failed event={event:?} uri={uri}: {err}");
        false
    });
}

fn open_markdown_link(parent: &gtk::Widget, uri: &str) {
    let uri = uri.trim().to_string();
    if uri.is_empty() {
        return;
    }

    let scheme = uri
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .unwrap_or_default();
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto" | "file"
    ) {
        log::debug!("markdown preview link ignored uri={uri}");
        return;
    }

    let parent_window = parent
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());
    let launch_parent = parent_window.clone();

    let dialog = adw::AlertDialog::builder()
        .heading("Open Link?")
        .body(&uri)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("open", "Open");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.choose(
        parent_window.as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response.as_str() != "open" {
                log::debug!("markdown preview link open cancelled uri={uri}");
                return;
            }

            let launch_context = launch_parent
                .as_ref()
                .map(|window| gtk::prelude::WidgetExt::display(window).app_launch_context());
            if let Err(err) =
                gtk::gio::AppInfo::launch_default_for_uri(&uri, launch_context.as_ref())
            {
                log::warn!("failed to open markdown preview link uri={uri}: {err}");
            }
        },
    );
}

fn base_uri_for_path(path: Option<&Path>) -> Option<String> {
    let base_dir = path.and_then(Path::parent).or(path)?;
    let mut uri = gtk::gio::File::for_path(base_dir).uri().to_string();
    if !uri.ends_with('/') {
        uri.push('/');
    }
    Some(uri)
}
