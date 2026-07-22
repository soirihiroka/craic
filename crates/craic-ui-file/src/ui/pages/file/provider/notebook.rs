use super::{PreviewMatchRequest, PreviewRequest};
use crate::config;
use crate::ui::content::code_editor;
use adw::prelude::*;
use craic_ui_preview::markdown_preview_web;
use std::cell::RefCell;
use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;
use webkit6::prelude::*;

const NOTEBOOK_POLL_MS: u64 = 80;
const JUPYTER_READY_TIMEOUT: Duration = Duration::from_secs(45);
const NOTEBOOK_INJECTED_CSS: &str = r#"
(() => {
  if (window.__craicNotebookChromeInstalled) return;
  window.__craicNotebookChromeInstalled = true;

  const style = document.createElement("style");
  style.textContent = `
    :root {
      color-scheme: light;
      --craic-window-bg: #fafafa;
      --craic-view-bg: #ffffff;
      --craic-headerbar-bg: #ebebeb;
      --craic-sidebar-bg: #f2f2f2;
      --craic-card-bg: #ffffff;
      --craic-fg: #2e3436;
      --craic-dim-fg: #5e5c64;
      --craic-border: rgb(0 0 0 / 14%);
      --craic-shade: rgb(0 0 0 / 9%);
      --craic-hover: rgb(0 0 0 / 7%);
      --craic-active: rgb(0 0 0 / 11%);
      --craic-accent: #3584e4;
      --craic-accent-bg: #3584e4;
      --craic-accent-fg: #ffffff;
      --craic-success: #2ec27e;
      --craic-warning: #e5a50a;
      --craic-error: #e01b24;
      --craic-radius: 12px;
      --craic-control-radius: 9px;

      --jp-layout-color0: var(--craic-window-bg);
      --jp-layout-color1: var(--craic-view-bg);
      --jp-layout-color2: var(--craic-sidebar-bg);
      --jp-layout-color3: var(--craic-headerbar-bg);
      --jp-layout-color4: var(--craic-card-bg);
      --jp-ui-font-color0: var(--craic-fg);
      --jp-ui-font-color1: var(--craic-fg);
      --jp-ui-font-color2: var(--craic-dim-fg);
      --jp-ui-font-color3: var(--craic-dim-fg);
      --jp-border-color0: var(--craic-border);
      --jp-border-color1: var(--craic-border);
      --jp-border-color2: var(--craic-border);
      --jp-border-color3: var(--craic-border);
      --jp-brand-color0: var(--craic-accent);
      --jp-brand-color1: var(--craic-accent-bg);
      --jp-brand-color2: color-mix(in srgb, var(--craic-accent-bg) 70%, var(--craic-view-bg));
      --jp-brand-color3: color-mix(in srgb, var(--craic-accent-bg) 35%, var(--craic-view-bg));
      --jp-accent-color0: var(--craic-accent);
      --jp-accent-color1: var(--craic-accent-bg);
      --jp-accent-color2: color-mix(in srgb, var(--craic-accent-bg) 70%, var(--craic-view-bg));
      --jp-accent-color3: color-mix(in srgb, var(--craic-accent-bg) 35%, var(--craic-view-bg));
      --jp-warn-color0: var(--craic-warning);
      --jp-error-color0: var(--craic-error);
      --jp-success-color0: var(--craic-success);
      --jp-cell-editor-background: color-mix(in srgb, var(--craic-view-bg) 94%, var(--craic-fg));
      --jp-cell-editor-border-color: var(--craic-border);
      --jp-cell-prompt-width: 68px;
      --jp-notebook-padding: 20px;
      --jp-rendermime-error-background: color-mix(in srgb, var(--craic-error) 12%, var(--craic-view-bg));
    }

    @media (prefers-color-scheme: dark) {
      :root {
        color-scheme: dark;
        --craic-window-bg: #1e1e1e;
        --craic-view-bg: #242424;
        --craic-headerbar-bg: #303030;
        --craic-sidebar-bg: #292929;
        --craic-card-bg: #2e2e2e;
        --craic-fg: #eeeeec;
        --craic-dim-fg: #c0bfbc;
        --craic-border: rgb(255 255 255 / 13%);
        --craic-shade: rgb(0 0 0 / 38%);
        --craic-hover: rgb(255 255 255 / 7%);
        --craic-active: rgb(255 255 255 / 11%);
        --craic-accent: #78aeed;
        --craic-accent-bg: #3584e4;
        --craic-accent-fg: #ffffff;
        --craic-success: #57e389;
        --craic-warning: #f8e45c;
        --craic-error: #ff7b63;
        --jp-cell-editor-background: #1f1f1f;
        --jp-rendermime-error-background: color-mix(in srgb, var(--craic-error) 18%, var(--craic-view-bg));
      }
    }

    html, body, #main, #jp-main-content-panel {
      min-height: 100%;
      background: var(--craic-window-bg) !important;
      color: var(--craic-fg);
    }

    body {
      overflow: auto;
      font-family: Cantarell, "Adwaita Sans", sans-serif;
    }

    #jp-top-panel,
    .jp-Toolbar,
    .jp-MenuBar,
    .lm-MenuBar,
    .jp-NotebookPanel-toolbar {
      background: var(--craic-headerbar-bg) !important;
      color: var(--craic-fg);
      border-color: var(--craic-border) !important;
      box-shadow: inset 0 -1px var(--craic-border);
    }

    #jp-left-stack,
    #jp-right-stack,
    .jp-SideBar,
    .jp-SidePanel,
    .jp-DirListing {
      background: var(--craic-sidebar-bg) !important;
      color: var(--craic-fg);
      border-color: var(--craic-border) !important;
    }

    .jp-ActivityItem,
    .jp-SideBar .lm-TabBar-tab,
    .jp-DirListing-item,
    .jp-RunningSessions-item {
      border-radius: var(--craic-control-radius);
      color: var(--craic-fg);
    }

    .jp-ActivityItem:hover,
    .jp-SideBar .lm-TabBar-tab:hover,
    .jp-DirListing-item:hover {
      background: var(--craic-hover);
    }

    .jp-ActivityItem.jp-mod-current,
    .jp-SideBar .lm-TabBar-tab.lm-mod-current,
    .jp-DirListing-item.jp-mod-selected {
      background: color-mix(in srgb, var(--craic-accent-bg) 22%, transparent);
      color: var(--craic-fg);
    }

    .lm-TabBar,
    .jp-DocumentSearch-overlay,
    .jp-Completer,
    .lm-Menu,
    .jp-Dialog-content {
      background: var(--craic-card-bg) !important;
      color: var(--craic-fg);
      border: 1px solid var(--craic-border);
      border-radius: var(--craic-radius);
      box-shadow: 0 2px 8px var(--craic-shade);
    }

    .lm-TabBar-tab,
    .jp-ToolbarButtonComponent,
    .jp-Button,
    button.jp-mod-styled {
      border-radius: var(--craic-control-radius) !important;
      color: var(--craic-fg) !important;
      background: transparent;
      border-color: transparent !important;
    }

    .lm-TabBar-tab:hover,
    .jp-ToolbarButtonComponent:hover,
    .jp-Button:hover,
    button.jp-mod-styled:hover {
      background: var(--craic-hover) !important;
    }

    .lm-TabBar-tab.lm-mod-current,
    .jp-ToolbarButtonComponent:active,
    .jp-Button:active,
    button.jp-mod-styled:active {
      background: var(--craic-active) !important;
    }

    .jp-Dialog-button.jp-mod-accept,
    button.jp-mod-accept,
    .jp-mod-accept.jp-Button {
      background: var(--craic-accent-bg) !important;
      color: var(--craic-accent-fg) !important;
    }

    .jp-Notebook {
      padding: 20px 24px 48px;
      background: var(--craic-window-bg);
    }

    .jp-NotebookPanel-notebook,
    .jp-NotebookPanel,
    .jp-MainAreaWidget,
    .jp-NotebookPanel .jp-Notebook {
      background: var(--craic-window-bg) !important;
    }

    .jp-Cell {
      margin: 10px 0;
      border-radius: var(--craic-radius);
      background: var(--craic-card-bg);
      border: 1px solid var(--craic-border);
      box-shadow: 0 1px 2px var(--craic-shade);
      overflow: hidden;
    }

    .jp-Cell.jp-mod-active,
    .jp-Cell.jp-mod-selected {
      border-color: color-mix(in srgb, var(--craic-accent-bg) 55%, var(--craic-border));
      box-shadow: 0 0 0 2px color-mix(in srgb, var(--craic-accent-bg) 22%, transparent),
                  0 1px 2px var(--craic-shade);
    }

    .jp-InputArea,
    .jp-OutputArea-child {
      background: transparent;
    }

    .jp-InputPrompt,
    .jp-OutputPrompt {
      color: var(--craic-dim-fg);
      font-weight: 600;
    }

    .jp-CodeCell .jp-InputArea-editor {
      background: var(--jp-cell-editor-background);
      border: 0;
      border-radius: 0;
    }

    .jp-RenderedHTMLCommon,
    .jp-MarkdownOutput,
    .jp-OutputArea-output {
      color: var(--craic-fg);
    }

    .jp-RenderedHTMLCommon code,
    .jp-RenderedHTMLCommon pre,
    .cm-editor,
    .cm-gutters {
      background: var(--jp-cell-editor-background) !important;
      color: var(--craic-fg);
    }

    .cm-gutters,
    .jp-Cell-inputWrapper,
    .jp-Cell-outputWrapper {
      border-color: var(--craic-border) !important;
    }

    .jp-StatusBar,
    .jp-PropertyInspector,
    .jp-FileBrowser-toolbar {
      background: var(--craic-headerbar-bg) !important;
      border-color: var(--craic-border) !important;
    }

    input,
    select,
    textarea,
    .jp-InputGroup input {
      background: var(--craic-view-bg) !important;
      color: var(--craic-fg) !important;
      border: 1px solid var(--craic-border) !important;
      border-radius: var(--craic-control-radius) !important;
    }

    input:focus,
    textarea:focus,
    .cm-editor.cm-focused {
      outline: 2px solid color-mix(in srgb, var(--craic-accent-bg) 55%, transparent) !important;
      outline-offset: -2px;
    }

    .jp-ToolbarButtonComponent-icon,
    .jp-icon3[fill],
    .jp-icon4[fill],
    .jp-icon-selectable[fill] {
      color: currentColor;
      fill: currentColor;
    }

    .jp-mod-running .jp-DirListing-itemIcon,
    .jp-RunningSessions-itemLabel {
      color: var(--craic-success);
    }

    .jp-mod-warn,
    .jp-Notification.jp-mod-warn {
      color: var(--craic-warning);
    }

    .jp-mod-error,
    .jp-Notification.jp-mod-error {
      color: var(--craic-error);
    }

    #header,
    #menubar,
    #maintoolbar,
    #header-container,
    #notebook_name,
    #kernel_indicator,
    #kernel_logo_widget,
    #login_widget,
    .header-bar,
    div#notebook_panel,
    div#notebook {
      background: var(--craic-window-bg) !important;
      color: var(--craic-fg);
    }

    #ipython_notebook,
    #kernel_logo_widget,
    #jp-MainLogo,
    #jp-MainLogo svg,
    svg.jp-JupyterIcon,
    .jp-JupyterIcon,
    .jp-JupyterIcon svg,
    [data-icon="jupyter"],
    [data-icon="jupyter"] svg,
    img[title^="Python"],
    img[src*="/kernelspecs/"][src*="logo"],
    jp-button[data-command="jupyter-notebook:open-lab"],
    button[data-command="jupyter-notebook:open-lab"],
    .jp-NotebookPanel-toolbar .jp-Toolbar-item:has(.jp-NotebookTrustedStatus),
    .jp-NotebookPanel-toolbar .jp-Toolbar-item:has(.jp-KernelName),
    .jp-NotebookPanel-toolbar .jp-Toolbar-item:has(.jp-Toolbar-kernelName),
    .jp-NotebookPanel-toolbar .jp-Toolbar-item:has(.jp-Toolbar-kernelStatus) {
      display: none !important;
    }

    #header {
      border-bottom: 1px solid var(--craic-border);
    }

    #menubar,
    #maintoolbar {
      background: var(--craic-headerbar-bg) !important;
      box-shadow: inset 0 -1px var(--craic-border);
    }

    #notebook-container {
      background: var(--craic-window-bg) !important;
      box-shadow: none !important;
      padding: 20px 24px 48px !important;
      width: auto !important;
    }

    div.cell {
      background: var(--craic-card-bg) !important;
      border: 1px solid var(--craic-border) !important;
      border-radius: var(--craic-radius) !important;
      box-shadow: 0 1px 2px var(--craic-shade);
      margin: 10px 0 !important;
      overflow: hidden;
    }

    div.cell.selected,
    div.cell.selected.jupyter-soft-selected {
      border-color: color-mix(in srgb, var(--craic-accent-bg) 55%, var(--craic-border)) !important;
      box-shadow: 0 0 0 2px color-mix(in srgb, var(--craic-accent-bg) 22%, transparent),
                  0 1px 2px var(--craic-shade) !important;
    }

    div.input_area,
    div.output_area pre,
    .CodeMirror {
      background: #f6f5f4 !important;
      color: var(--craic-fg) !important;
      border-color: var(--craic-border) !important;
    }

    @media (prefers-color-scheme: dark) {
      div.input_area,
      div.output_area pre,
      .CodeMirror {
        background: #242424 !important;
      }
    }

    div.input_area {
      border-radius: 0 !important;
      border: 0 !important;
    }

    .CodeMirror,
    .CodeMirror pre,
    .CodeMirror-lines,
    .cm-editor,
    .cm-content,
    .cm-line {
      font-family: "Adwaita Mono", "Cascadia Code", "Source Code Pro", monospace !important;
      color: var(--craic-fg) !important;
    }

    .CodeMirror-gutters,
    .cm-gutters {
      background: color-mix(in srgb, var(--jp-cell-editor-background) 90%, var(--craic-window-bg)) !important;
      border-color: var(--craic-border) !important;
      color: var(--craic-dim-fg) !important;
    }

    .CodeMirror-cursor,
    .cm-cursor {
      border-left-color: var(--craic-fg) !important;
    }

    .CodeMirror-selected,
    .cm-selectionBackground,
    .cm-content ::selection {
      background: color-mix(in srgb, var(--craic-accent-bg) 34%, transparent) !important;
    }

    div.prompt,
    div.input_prompt,
    div.output_prompt {
      color: var(--craic-dim-fg) !important;
      font-weight: 600;
    }

    .navbar,
    .navbar-default,
    .dropdown-menu,
    .modal-content,
    .popover,
    .notification_widget {
      background: var(--craic-card-bg) !important;
      color: var(--craic-fg) !important;
      border-color: var(--craic-border) !important;
      border-radius: var(--craic-radius) !important;
      box-shadow: 0 2px 8px var(--craic-shade) !important;
    }

    .btn,
    .btn-default,
    .toolbar-btn,
    button {
      border-radius: var(--craic-control-radius) !important;
      border-color: transparent !important;
      background: transparent !important;
      color: var(--craic-fg) !important;
    }

    .btn:hover,
    .btn-default:hover,
    .toolbar-btn:hover,
    button:hover {
      background: var(--craic-hover) !important;
    }

    .btn-primary,
    .btn-success {
      background: var(--craic-accent-bg) !important;
      color: var(--craic-accent-fg) !important;
    }

    .jp-NotebookPanel-toolbar .jp-Toolbar-spacer,
    #notification_trusted,
    #kernel_indicator_icon,
    #kernel_indicator_name {
      display: none !important;
    }
  `;
  document.documentElement.appendChild(style);

  const titleForButton = (button) => {
    const label = button.getAttribute("title") || button.getAttribute("aria-label") || "";
    return label.toLowerCase();
  };

  const replacements = [
    ["save", "document-save-symbolic"],
    ["cut", "edit-cut-symbolic"],
    ["copy", "edit-copy-symbolic"],
    ["paste", "edit-paste-symbolic"],
    ["run", "media-playback-start-symbolic"],
    ["restart", "view-refresh-symbolic"],
    ["interrupt", "process-stop-symbolic"],
    ["add", "list-add-symbolic"],
    ["delete", "edit-delete-symbolic"]
  ];

  const applyIconHints = () => {
    for (const button of document.querySelectorAll("button")) {
      const label = titleForButton(button);
      const match = replacements.find(([needle]) => label.includes(needle));
      if (!match) continue;
      button.setAttribute("data-craic-adw-icon", match[1]);
    }
  };

  applyIconHints();
  new MutationObserver(applyIconHints).observe(document.documentElement, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["title", "aria-label"]
  });
})();
"#;

pub struct NotebookPreview {
    pub root: gtk::Stack,
    web_view: webkit6::WebView,
    readonly_list: gtk::Box,
    server: RefCell<Option<NotebookServer>>,
    allowed_origin: Rc<RefCell<Option<String>>>,
}

struct NotebookServer {
    repo_path: PathBuf,
    venv_python: PathBuf,
    origin: String,
    token: String,
    child: Child,
}

impl Drop for NotebookServer {
    fn drop(&mut self) {
        log::info!(
            "jupyter notebook server stopping repo_path={} origin={}",
            self.repo_path.display(),
            self.origin
        );
        if let Err(err) = self.child.kill() {
            log::debug!("jupyter notebook server kill skipped: {err}");
        }
    }
}

impl NotebookPreview {
    pub fn new() -> Rc<Self> {
        let user_content_manager = webkit6::UserContentManager::new();
        user_content_manager.add_script(&webkit6::UserScript::new(
            NOTEBOOK_INJECTED_CSS,
            webkit6::UserContentInjectedFrames::TopFrame,
            webkit6::UserScriptInjectionTime::End,
            &[],
            &[],
        ));

        let web_view = webkit6::WebView::builder()
            .user_content_manager(&user_content_manager)
            .build();
        web_view.set_hexpand(true);
        web_view.set_vexpand(true);

        let allowed_origin = Rc::new(RefCell::new(None::<String>));
        install_navigation_policy(&web_view, Rc::clone(&allowed_origin));
        install_load_logging(&web_view);

        let readonly_list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(16)
            .margin_bottom(32)
            .margin_start(20)
            .margin_end(20)
            .hexpand(true)
            .vexpand(false)
            .build();
        let readonly_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&readonly_list)
            .build();

        let root = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        root.set_hhomogeneous(false);
        root.set_vhomogeneous(false);
        root.add_named(&web_view, Some("live"));
        root.add_named(&readonly_scroller, Some("readonly"));

        Rc::new(Self {
            root,
            web_view,
            readonly_list,
            server: RefCell::new(None),
            allowed_origin,
        })
    }

    pub fn clear(&self) {
        self.clear_readonly_cells();
    }

    fn reuse_url(
        &self,
        repo_path: &Path,
        venv_python: &Path,
        notebook_path: &str,
    ) -> Option<String> {
        let server = self.server.borrow();
        let server = server.as_ref()?;
        if server.repo_path == repo_path && server.venv_python == venv_python {
            return Some(notebook_url(&server.origin, notebook_path, &server.token));
        }
        None
    }

    fn load_notebook(&self, server: NotebookServer, notebook_path: &str) {
        let origin = server.origin.clone();
        let url = notebook_url(&origin, notebook_path, &server.token);
        self.allowed_origin.replace(Some(origin));
        self.server.replace(Some(server));
        self.root.set_visible_child_name("live");
        self.web_view.load_uri(&url);
    }

    fn load_reused_notebook(&self, url: &str) {
        self.clear_readonly_cells();
        self.root.set_visible_child_name("live");
        self.web_view.load_uri(url);
    }

    pub fn load_readonly_cells(
        &self,
        cells: Vec<super::notebook_readonly::RenderCell>,
        path: Option<&Path>,
    ) {
        self.allowed_origin.replace(None);
        self.server.replace(None);
        self.clear_readonly_cells();

        let base_uri = path
            .and_then(|path| path.parent())
            .map(gtk::gio::File::for_path)
            .map(|file| {
                let mut uri = file.uri().to_string();
                if !uri.ends_with('/') {
                    uri.push('/');
                }
                uri
            });

        log::info!(
            "readonly notebook native render start path={} cells={}",
            path.map(|path| path.display().to_string())
                .unwrap_or_else(|| "provider".to_string()),
            cells.len()
        );
        if cells.is_empty() {
            let label = gtk::Label::builder()
                .label("This notebook has no cells.")
                .halign(gtk::Align::Start)
                .css_classes(["dim-label"])
                .build();
            self.readonly_list.append(&label);
        }

        for (index, cell) in cells.into_iter().enumerate() {
            log::info!(
                "readonly notebook native cell build index={index} kind={} bytes={}",
                cell.kind(),
                cell.source_len()
            );
            match cell {
                super::notebook_readonly::RenderCell::Markdown { source } => {
                    self.readonly_list
                        .append(&readonly_markdown_cell(&source, base_uri.as_deref()));
                }
                super::notebook_readonly::RenderCell::Code {
                    execution_count,
                    source,
                } => {
                    self.readonly_list
                        .append(&readonly_code_cell(execution_count, &source));
                }
                super::notebook_readonly::RenderCell::Raw { source } => {
                    self.readonly_list
                        .append(&readonly_raw_cell(index, &source));
                }
            }
        }
        self.root.set_visible_child_name("readonly");
        log::info!(
            "readonly notebook native render complete path={}",
            path.map(|path| path.display().to_string())
                .unwrap_or_else(|| "provider".to_string())
        );
    }

    fn clear_readonly_cells(&self) {
        while let Some(child) = self.readonly_list.first_child() {
            self.readonly_list.remove(&child);
        }
    }
}

fn readonly_markdown_cell(source: &str, base_uri: Option<&str>) -> gtk::Widget {
    let height = markdown_height_guess(source);
    log::info!(
        "readonly notebook markdown widget bytes={} visible_lines={} height={height}",
        source.len(),
        visible_markdown_line_count(source)
    );
    let html = markdown_preview_web::markdown_document_html(source);
    let web_view = webkit6::WebView::new();
    web_view.set_hexpand(true);
    web_view.set_vexpand(false);
    web_view.set_size_request(-1, height);
    web_view.load_html(&html, base_uri);

    readonly_card(&web_view)
}

fn readonly_code_cell(execution_count: Option<i32>, source: &str) -> gtk::Widget {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .build();
    let prompt = gtk::Label::builder()
        .label(format!(
            "[{}]:",
            execution_count
                .map(|count| count.to_string())
                .unwrap_or_default()
        ))
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .width_chars(4)
        .margin_top(10)
        .margin_start(6)
        .margin_end(6)
        .css_classes(["dim-label"])
        .build();
    let editor = code_editor::CodeEditor::new("python", source);
    editor.set_font_size(config::load().font_sizes.editor);
    editor.set_editable(false);
    editor.root.set_hexpand(true);
    editor.root.set_vexpand(false);
    editor.root.set_size_request(-1, code_height_guess(source));
    row.append(&prompt);
    row.append(&editor.root);

    readonly_card(&row)
}

fn readonly_raw_cell(index: usize, source: &str) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(format!("Raw [{}]:\n{}", index + 1, source))
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    readonly_card(&label)
}

fn readonly_card(child: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let frame = gtk::Frame::builder()
        .child(child)
        .hexpand(true)
        .vexpand(false)
        .build();
    frame.add_css_class("card");
    frame.upcast()
}

fn markdown_height_guess(source: &str) -> i32 {
    let lines = visible_markdown_line_count(source).max(1) as i32;
    (lines * 26 + 48).clamp(90, 900)
}

fn visible_markdown_line_count(source: &str) -> usize {
    let mut in_comment = false;
    let mut visible = 0;

    for line in source.lines() {
        let mut rest = line;
        loop {
            if in_comment {
                if let Some(end) = rest.find("-->") {
                    rest = &rest[end + 3..];
                    in_comment = false;
                } else {
                    rest = "";
                }
            }

            if let Some(start) = rest.find("<!--") {
                if !rest[..start].trim().is_empty() {
                    visible += 1;
                }
                rest = &rest[start + 4..];
                if let Some(end) = rest.find("-->") {
                    rest = &rest[end + 3..];
                    continue;
                }
                in_comment = true;
                break;
            }

            if !rest.trim().is_empty() {
                visible += 1;
            }
            break;
        }
    }

    visible
}

fn code_height_guess(source: &str) -> i32 {
    let lines = source.lines().count().max(1) as i32;
    (lines * 22 + 28).clamp(96, 900)
}

pub fn show(request: PreviewRequest<'_>) {
    show_notebook(request);
}

pub fn show_match(request: PreviewMatchRequest<'_>) {
    show_notebook(request.into_preview_request());
}

#[derive(Clone)]
struct ReadonlyNotebookSource {
    local_path: Option<PathBuf>,
    prefetched_bytes: Option<Vec<u8>>,
}

impl ReadonlyNotebookSource {
    fn show(
        &self,
        right: Rc<super::right::RightPane>,
        load_token: super::right::PreviewLoadToken,
        file_path: String,
    ) {
        super::notebook_readonly::show(
            right,
            load_token,
            file_path,
            self.local_path.clone(),
            self.prefetched_bytes.clone(),
        );
    }
}

fn show_notebook(request: PreviewRequest<'_>) {
    request.right.show_provider_loading_message(
        request.load_token,
        request.file_path,
        "Checking notebook environment...",
    );

    let readonly_source = ReadonlyNotebookSource {
        local_path: request.local_path.map(Path::to_path_buf),
        prefetched_bytes: request.prefetched_bytes.map(|bytes| bytes.to_vec()),
    };
    if readonly_source.local_path.is_none() {
        log::info!(
            "jupyter notebook live preview unavailable without local path; showing readonly file_path={}",
            request.file_path
        );
        readonly_source.show(
            Rc::clone(&request.right),
            request.load_token,
            request.file_path.to_string(),
        );
        return;
    }

    let Some(repo_path) = request.ctx.local_workspace_path() else {
        log::info!(
            "jupyter notebook live preview unavailable without local workspace path; showing readonly file_path={}",
            request.file_path
        );
        readonly_source.show(
            Rc::clone(&request.right),
            request.load_token,
            request.file_path.to_string(),
        );
        return;
    };
    let file_path = request.file_path.to_string();
    let right = Rc::clone(&request.right);
    let load_token = request.load_token;
    let ctx = request.ctx.clone();
    let (sender, receiver) = mpsc::channel();

    thread::spawn({
        let repo_path = repo_path.clone();
        move || {
            let result = inspect_environment(&repo_path);
            let _ = sender.send(result);
        }
    });

    receive_environment_result(
        ctx,
        right,
        load_token,
        repo_path,
        file_path,
        readonly_source,
        receiver,
    );
}

fn receive_environment_result(
    ctx: super::PageContext,
    right: Rc<super::right::RightPane>,
    load_token: super::right::PreviewLoadToken,
    repo_path: PathBuf,
    file_path: String,
    readonly_source: ReadonlyNotebookSource,
    receiver: mpsc::Receiver<Result<EnvironmentState, String>>,
) {
    gtk::glib::timeout_add_local(
        Duration::from_millis(NOTEBOOK_POLL_MS),
        move || match receiver.try_recv() {
            Ok(Ok(EnvironmentState::Ready { venv_python })) => {
                if right.is_current_load(load_token) {
                    ask_open_notebook_mode(
                        ctx.clone(),
                        Rc::clone(&right),
                        load_token,
                        repo_path.clone(),
                        file_path.clone(),
                        readonly_source.clone(),
                        venv_python,
                    );
                }
                gtk::glib::ControlFlow::Break
            }
            Ok(Ok(EnvironmentState::NeedsInitialization { reason })) => {
                if right.is_current_load(load_token) {
                    log::info!("jupyter notebook initialization required reason={reason}");
                    ask_initialize_notebook(
                        ctx.clone(),
                        Rc::clone(&right),
                        load_token,
                        repo_path.clone(),
                        file_path.clone(),
                        readonly_source.clone(),
                        reason,
                    );
                }
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                log::warn!("jupyter notebook environment inspection failed: {err}");
                if right.is_current_load(load_token) {
                    right.show_unavailable(&file_path, &err);
                    ctx.show_error("Notebook Preview Failed", &popup_error_message(&err));
                }
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                if right.is_current_load(load_token) {
                    let message = "Notebook environment check did not return a result.";
                    right.show_unavailable(&file_path, message);
                    ctx.show_error("Notebook Preview Failed", message);
                }
                gtk::glib::ControlFlow::Break
            }
        },
    );
}

fn ask_open_notebook_mode(
    ctx: super::PageContext,
    right: Rc<super::right::RightPane>,
    load_token: super::right::PreviewLoadToken,
    repo_path: PathBuf,
    file_path: String,
    readonly_source: ReadonlyNotebookSource,
    venv_python: PathBuf,
) {
    let Some(window) = ctx.window() else {
        log::info!(
            "jupyter notebook mode prompt unavailable; showing readonly file_path={file_path}"
        );
        readonly_source.show(right, load_token, file_path);
        return;
    };

    right.show_provider_loading_message(
        load_token,
        &file_path,
        "Choose how to open this notebook: live Jupyter Notebook or read-only preview.",
    );

    let dialog = adw::AlertDialog::new(
        Some("Open Notebook"),
        Some(
            "Open with a live Jupyter Notebook kernel, or use Craic's read-only preview without starting Jupyter.",
        ),
    );
    dialog.add_response("readonly", "Read-only Preview");
    dialog.add_response("live", "Live Notebook");
    dialog.set_default_response(Some("readonly"));
    dialog.set_close_response("readonly");
    dialog.set_response_appearance("live", adw::ResponseAppearance::Suggested);

    dialog.connect_response(None, move |_, response| {
        if response != "live" {
            log::info!("jupyter notebook readonly mode selected file_path={file_path}");
            readonly_source.show(Rc::clone(&right), load_token, file_path.clone());
            return;
        }

        log::info!("jupyter notebook live mode selected file_path={file_path}");
        if let Some(url) =
            right
                .file_notebook_preview
                .reuse_url(&repo_path, &venv_python, &file_path)
        {
            log::info!("jupyter notebook preview reusing notebook server file_path={file_path}");
            right.show_notebook_preview(&file_path);
            right.file_notebook_preview.load_reused_notebook(&url);
        } else {
            right.show_provider_loading_message(
                load_token,
                &file_path,
                "Starting Jupyter Notebook server...",
            );
            launch_server_for_preview(
                ctx.clone(),
                Rc::clone(&right),
                load_token,
                repo_path.clone(),
                file_path.clone(),
                venv_python.clone(),
            );
        }
    });
    dialog.present(Some(&window));
}

fn ask_initialize_notebook(
    ctx: super::PageContext,
    right: Rc<super::right::RightPane>,
    load_token: super::right::PreviewLoadToken,
    repo_path: PathBuf,
    file_path: String,
    readonly_source: ReadonlyNotebookSource,
    reason: String,
) {
    let Some(window) = ctx.window() else {
        log::info!(
            "jupyter notebook initialization unavailable; showing readonly file_path={file_path}"
        );
        readonly_source.show(right, load_token, file_path);
        return;
    };

    right.show_provider_loading_message(
        load_token,
        &file_path,
        &format!("{reason}\n\nInitialize notebook support to open this file."),
    );

    let dialog = adw::AlertDialog::new(
        Some("Initialize Notebook Kernel?"),
        Some(&format!(
            "{reason}\n\nCraic can create/use a local venv and install Jupyter Notebook and ipykernel with uv."
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("initialize", "Initialize");
    dialog.set_default_response(Some("initialize"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("initialize", adw::ResponseAppearance::Suggested);

    dialog.connect_response(None, move |_, response| {
        if response != "initialize" {
            log::info!(
                "jupyter notebook initialization declined; showing readonly file_path={file_path}"
            );
            readonly_source.show(Rc::clone(&right), load_token, file_path.clone());
            return;
        }

        log::info!("jupyter notebook initialization accepted file_path={file_path}");
        right.show_provider_loading_message(
            load_token,
            &file_path,
            "Initializing notebook support...",
        );
        initialize_then_launch(
            ctx.clone(),
            Rc::clone(&right),
            load_token,
            repo_path.clone(),
            file_path.clone(),
        );
    });
    dialog.present(Some(&window));
}

fn initialize_then_launch(
    ctx: super::PageContext,
    right: Rc<super::right::RightPane>,
    load_token: super::right::PreviewLoadToken,
    repo_path: PathBuf,
    file_path: String,
) {
    let (sender, receiver) = mpsc::channel();
    let launch_file_path = file_path.clone();
    thread::spawn({
        let repo_path = repo_path.clone();
        move || {
            let result = initialize_environment(&repo_path).and_then(|venv_python| {
                launch_jupyter_notebook(&repo_path, &venv_python, &launch_file_path)
            });
            let _ = sender.send(result);
        }
    });

    receive_launch_result(ctx, right, load_token, file_path, receiver);
}

fn launch_server_for_preview(
    ctx: super::PageContext,
    right: Rc<super::right::RightPane>,
    load_token: super::right::PreviewLoadToken,
    repo_path: PathBuf,
    file_path: String,
    venv_python: PathBuf,
) {
    let (sender, receiver) = mpsc::channel();
    let launch_file_path = file_path.clone();
    thread::spawn(move || {
        let result = launch_jupyter_notebook(&repo_path, &venv_python, &launch_file_path);
        let _ = sender.send(result);
    });

    receive_launch_result(ctx, right, load_token, file_path, receiver);
}

fn receive_launch_result(
    ctx: super::PageContext,
    right: Rc<super::right::RightPane>,
    load_token: super::right::PreviewLoadToken,
    file_path: String,
    receiver: mpsc::Receiver<Result<NotebookServer, String>>,
) {
    gtk::glib::timeout_add_local(
        Duration::from_millis(NOTEBOOK_POLL_MS),
        move || match receiver.try_recv() {
            Ok(Ok(server)) => {
                if right.is_current_load(load_token) {
                    log::info!("jupyter notebook preview ready file_path={file_path}");
                    right.show_provider_loading_message(
                        load_token,
                        &file_path,
                        "Loading notebook in embedded browser...",
                    );
                    right.show_notebook_preview(&file_path);
                    right
                        .file_notebook_preview
                        .load_notebook(server, &file_path);
                }
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                log::warn!("jupyter notebook preview failed file_path={file_path}: {err}");
                if right.is_current_load(load_token) {
                    right.show_unavailable(&file_path, &err);
                    ctx.show_error("Notebook Preview Failed", &popup_error_message(&err));
                }
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                if right.is_current_load(load_token) {
                    let message = "Notebook preview did not return a result.";
                    right.show_unavailable(&file_path, message);
                    ctx.show_error("Notebook Preview Failed", message);
                }
                gtk::glib::ControlFlow::Break
            }
        },
    );
}

enum EnvironmentState {
    Ready { venv_python: PathBuf },
    NeedsInitialization { reason: String },
}

fn inspect_environment(repo_path: &Path) -> Result<EnvironmentState, String> {
    ensure_uv_available()?;

    let Some(venv_python) = find_venv_python(repo_path) else {
        return Ok(EnvironmentState::NeedsInitialization {
            reason: "No repo-local Python venv was found.".to_string(),
        });
    };

    if !python_imports(&venv_python, &["notebook", "ipykernel"])? {
        return Ok(EnvironmentState::NeedsInitialization {
            reason: "The local venv is missing Jupyter Notebook or ipykernel.".to_string(),
        });
    }

    Ok(EnvironmentState::Ready { venv_python })
}

fn initialize_environment(repo_path: &Path) -> Result<PathBuf, String> {
    ensure_uv_available()?;

    let venv_python = match find_venv_python(repo_path) {
        Some(path) => path,
        None => {
            log::info!(
                "jupyter notebook creating venv repo_path={}",
                repo_path.display()
            );
            run_command(
                Command::new("uv")
                    .arg("venv")
                    .arg(".venv")
                    .current_dir(repo_path),
                "create Python venv",
            )?;
            repo_path.join(".venv").join("bin").join("python")
        }
    };

    log::info!(
        "jupyter notebook installing packages python={}",
        venv_python.display()
    );
    run_command(
        Command::new("uv")
            .arg("pip")
            .arg("install")
            .arg("--python")
            .arg(&venv_python)
            .arg("notebook")
            .arg("ipykernel")
            .current_dir(repo_path),
        "install notebook packages",
    )?;

    let kernel_name = kernel_name_for_repo(repo_path);
    let display_name = format!(
        "Python ({})",
        repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".venv")
    );
    log::info!("jupyter notebook registering kernel name={kernel_name}");
    run_command(
        Command::new(&venv_python)
            .arg("-m")
            .arg("ipykernel")
            .arg("install")
            .arg("--user")
            .arg("--name")
            .arg(kernel_name)
            .arg("--display-name")
            .arg(display_name)
            .current_dir(repo_path),
        "register notebook kernel",
    )?;

    Ok(venv_python)
}

fn launch_jupyter_notebook(
    repo_path: &Path,
    venv_python: &Path,
    _notebook_path: &str,
) -> Result<NotebookServer, String> {
    let port = available_port()?;
    let origin = format!("http://127.0.0.1:{port}");
    let token = Uuid::new_v4().to_string();
    let runtime_dir =
        std::env::temp_dir().join(format!("craic-jupyter-notebook-{}", Uuid::new_v4()));
    fs::create_dir_all(&runtime_dir)
        .map_err(|err| format!("Unable to create Jupyter runtime directory: {err}"))?;
    let stdout_path = runtime_dir.join("jupyter-notebook.stdout.log");
    let stderr_path = runtime_dir.join("jupyter-notebook.stderr.log");
    let lab_settings_dir = write_jupyterlab_settings(&runtime_dir)?;
    let lab_workspaces_dir = runtime_dir.join("lab-workspaces");
    fs::create_dir_all(&lab_workspaces_dir)
        .map_err(|err| format!("Unable to create JupyterLab workspace directory: {err}"))?;

    log::info!(
        "jupyter notebook launching notebook server repo_path={} origin={origin} settings_dir={}",
        repo_path.display(),
        lab_settings_dir.display()
    );
    let stdout = File::create(&stdout_path)
        .map_err(|err| format!("Unable to create Jupyter Notebook stdout log: {err}"))?;
    let stderr = File::create(&stderr_path)
        .map_err(|err| format!("Unable to create Jupyter Notebook stderr log: {err}"))?;
    let mut child = Command::new(venv_python)
        .arg("-m")
        .arg("notebook")
        .arg("--no-browser")
        .arg("--ServerApp.ip=127.0.0.1")
        .arg(format!("--ServerApp.port={port}"))
        .arg("--ServerApp.port_retries=0")
        .arg(format!("--ServerApp.root_dir={}", repo_path.display()))
        .arg(format!("--ServerApp.token={token}"))
        .arg("--ServerApp.password=")
        .arg("--ServerApp.open_browser=False")
        .arg("--ServerApp.allow_remote_access=False")
        .current_dir(repo_path)
        .env("JUPYTERLAB_SETTINGS_DIR", &lab_settings_dir)
        .env("JUPYTERLAB_WORKSPACES_DIR", &lab_workspaces_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|err| format!("Unable to launch Jupyter Notebook: {err}"))?;

    if let Err(err) =
        wait_for_jupyter_notebook(&origin, &token, &mut child, &stdout_path, &stderr_path)
    {
        let _ = child.kill();
        return Err(err);
    }

    Ok(NotebookServer {
        repo_path: repo_path.to_path_buf(),
        venv_python: venv_python.to_path_buf(),
        origin,
        token,
        child,
    })
}

fn write_jupyterlab_settings(runtime_dir: &Path) -> Result<PathBuf, String> {
    let settings_dir = runtime_dir.join("lab-user-settings");
    let apputils_dir = settings_dir.join("@jupyterlab").join("apputils-extension");
    fs::create_dir_all(&apputils_dir)
        .map_err(|err| format!("Unable to create JupyterLab settings directory: {err}"))?;

    fs::write(
        apputils_dir.join("themes.jupyterlab-settings"),
        r#"{
  "adaptive-theme": true,
  "preferred-light-theme": "JupyterLab Light",
  "preferred-dark-theme": "JupyterLab Dark",
  "theme-scrollbars": true,
  "overrides": {
    "code-font-family": "Adwaita Mono, Cascadia Code, Source Code Pro, monospace",
    "content-font-family": "Cantarell, Adwaita Sans, sans-serif",
    "ui-font-family": "Cantarell, Adwaita Sans, sans-serif"
  }
}
"#,
    )
    .map_err(|err| format!("Unable to write JupyterLab theme settings: {err}"))?;

    Ok(settings_dir)
}

fn ensure_uv_available() -> Result<(), String> {
    match Command::new("uv").arg("--version").output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(_) => Err("uv is installed but did not run successfully.".to_string()),
        Err(err) => Err(format!("uv is required to initialize notebooks: {err}")),
    }
}

fn find_venv_python(repo_path: &Path) -> Option<PathBuf> {
    [".venv", "venv"]
        .into_iter()
        .map(|name| repo_path.join(name).join("bin").join("python"))
        .find(|path| path.is_file())
}

fn python_imports(python: &Path, modules: &[&str]) -> Result<bool, String> {
    let script = modules
        .iter()
        .map(|module| format!("import {module}"))
        .collect::<Vec<_>>()
        .join("; ");
    let output = Command::new(python)
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|err| format!("Unable to inspect Python environment: {err}"))?;
    Ok(output.status.success())
}

fn run_command(command: &mut Command, label: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|err| format!("Unable to {label}: {err}"))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(format!(
        "Failed to {label}.\n{}{}",
        stdout.trim(),
        stderr.trim()
    ))
}

fn available_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| format!("Unable to allocate localhost port: {err}"))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|err| format!("Unable to read allocated localhost port: {err}"))
}

fn wait_for_jupyter_notebook(
    origin: &str,
    token: &str,
    child: &mut Child,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(), String> {
    let deadline = Instant::now() + JUPYTER_READY_TIMEOUT;
    let status_url = format!("{origin}/api/status?token={}", percent_encode_query(token));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(900))
        .build()
        .map_err(|err| format!("Unable to create Jupyter readiness client: {err}"))?;

    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "Jupyter Notebook exited before it was ready: {status}\n{}",
                child_output(stdout_path, stderr_path)
            ));
        }
        if client
            .get(&status_url)
            .send()
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }

    let _ = child.kill();
    let _ = child.wait();
    Err(format!(
        "Timed out waiting for Jupyter Notebook to start.\n{}",
        child_output(stdout_path, stderr_path)
    ))
}

fn child_output(stdout_path: &Path, stderr_path: &Path) -> String {
    let mut output = String::new();

    if let Ok(text) = fs::read_to_string(stdout_path) {
        if !text.trim().is_empty() {
            append_log_excerpt(&mut output, "stdout", &text);
        }
    }

    if let Ok(text) = fs::read_to_string(stderr_path) {
        if !text.trim().is_empty() {
            append_log_excerpt(&mut output, "stderr", &text);
        }
    }

    if output.trim().is_empty() {
        "No Jupyter Notebook output was captured.".to_string()
    } else {
        output.trim().to_string()
    }
}

fn append_log_excerpt(output: &mut String, label: &str, text: &str) {
    const MAX_LOG_CHARS: usize = 8_000;
    output.push_str(label);
    output.push_str(":\n");
    let text = text.trim();
    if text.chars().count() > MAX_LOG_CHARS {
        output.push_str("[log truncated]\n");
        output.push_str(
            &text
                .chars()
                .rev()
                .take(MAX_LOG_CHARS)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>(),
        );
    } else {
        output.push_str(text);
    }
    output.push('\n');
}

fn notebook_url(origin: &str, notebook_path: &str, token: &str) -> String {
    format!(
        "{origin}/notebooks/{}?token={}",
        percent_encode_path(notebook_path),
        percent_encode_query(token)
    )
}

fn kernel_name_for_repo(repo_path: &Path) -> String {
    let name = repo_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("craic");
    let slug = name
        .chars()
        .map(|ch| {
            let ch = ch.to_ascii_lowercase();
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("craic-{slug}")
}

fn percent_encode_path(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn percent_encode_query(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn popup_error_message(message: &str) -> String {
    const MAX_CHARS: usize = 900;
    let mut text = message.trim().to_string();
    if text.chars().count() <= MAX_CHARS {
        return text;
    }

    text = text.chars().take(MAX_CHARS).collect::<String>();
    text.push_str("\n\nFull details are shown in the notebook preview pane.");
    text
}

fn install_navigation_policy(
    web_view: &webkit6::WebView,
    allowed_origin: Rc<RefCell<Option<String>>>,
) {
    web_view.connect_decide_policy(move |_, decision, decision_type| {
        let is_navigation = matches!(
            decision_type,
            webkit6::PolicyDecisionType::NavigationAction
                | webkit6::PolicyDecisionType::NewWindowAction
        );
        if !is_navigation {
            return false;
        }

        let Some(navigation_decision) =
            decision.downcast_ref::<webkit6::NavigationPolicyDecision>()
        else {
            decision.ignore();
            return true;
        };
        let uri = navigation_decision
            .navigation_action()
            .and_then(|action| action.request())
            .and_then(|request| request.uri())
            .map(|uri| uri.to_string())
            .unwrap_or_default();

        let allowed = uri.is_empty()
            || uri.starts_with("about:")
            || allowed_origin
                .borrow()
                .as_ref()
                .is_some_and(|origin| uri.starts_with(origin));
        if allowed {
            return false;
        }

        log::debug!("blocked notebook navigation uri={uri}");
        decision.ignore();
        true
    });
}

fn install_load_logging(web_view: &webkit6::WebView) {
    web_view.connect_load_changed(|web_view, event| {
        let uri = web_view
            .uri()
            .map(|uri| uri.to_string())
            .unwrap_or_default();
        log::debug!("jupyter notebook webview load event={event:?} uri={uri}");
        if matches!(event, webkit6::LoadEvent::Finished) {
            log::info!("jupyter notebook webview load finished uri={uri}");
        }
    });

    web_view.connect_load_failed(|_, event, uri, err| {
        log::warn!("jupyter notebook webview load failed event={event:?} uri={uri}: {err}");
        false
    });
}
