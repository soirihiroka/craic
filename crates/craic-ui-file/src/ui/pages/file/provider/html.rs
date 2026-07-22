use super::{PreviewMatchRequest, PreviewRequest};
use crate::git::FileComparison;
use adw::prelude::*;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use webkit6::prelude::*;

struct HtmlPreviewLoad {
    text: String,
    spellcheck_issues: Vec<crate::spellcheck::SpellcheckIssue>,
}

pub struct HtmlPreview {
    pub root: webkit6::WebView,
}

impl HtmlPreview {
    pub fn new() -> Rc<Self> {
        let root = webkit6::WebView::new();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_focusable(true);
        root.set_size_request(0, -1);

        if let Some(settings) = webkit6::prelude::WebViewExt::settings(&root) {
            settings.set_javascript_can_access_clipboard(false);
            settings.set_javascript_can_open_windows_automatically(false);
        }

        root.connect_load_failed(|_, event, uri, err| {
            log::warn!("html preview load failed event={event:?} uri={uri}: {err}");
            false
        });

        Rc::new(Self { root })
    }

    pub fn set_html(&self, html: &str, path: Option<&Path>) {
        log::debug!("loading html preview html_bytes={}", html.len());
        self.root
            .load_html(html, base_uri_for_path(path).as_deref());
    }
}

pub fn show(request: PreviewRequest<'_>) {
    show_html(request, None);
}

pub fn show_match(request: PreviewMatchRequest<'_>) {
    let selection = Some((request.start, request.end));
    show_html(request.into_preview_request(), selection);
}

fn show_html(request: PreviewRequest<'_>, selection: Option<(usize, usize)>) {
    request
        .right
        .show_editor_loading(request.load_token, request.file_path, "HTML");

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
    let comparison_right = Rc::clone(&request.right);
    let comparison_token = request.load_token;

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || {
            super::super::repository_text_from_prefetch(prefetched_bytes, &file_path).map(|text| {
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
                HtmlPreviewLoad {
                    text,
                    spellcheck_issues,
                }
            })
        },
        move |right, result| match result {
            Ok(load) => {
                if let Some(git) = git.clone() {
                    let (sender, receiver) = mpsc::channel();
                    git.comparison(
                        &apply_file_path,
                        Box::new(move |result| {
                            let _ = sender.send(result.ok());
                        }),
                    );
                    let node_path = apply_node_path.clone();
                    let file_path = apply_file_path.clone();
                    let preview_path = local_path.clone();
                    let mut load = Some(load);
                    super::receive_preview_load(
                        Rc::clone(&comparison_right),
                        comparison_token,
                        apply_file_path.clone(),
                        receiver,
                        move |right, comparison: Option<FileComparison>| {
                            let Some(load) = load.take() else {
                                return;
                            };
                            show_loaded_html(
                                right,
                                &node_path,
                                &file_path,
                                load,
                                disk_signature,
                                writable,
                                comparison.as_ref(),
                                preview_path.as_deref(),
                                selection,
                            );
                        },
                    );
                } else {
                    show_loaded_html(
                        right,
                        &apply_node_path,
                        &apply_file_path,
                        load,
                        disk_signature,
                        writable,
                        None,
                        local_path.as_deref(),
                        selection,
                    );
                }
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn show_loaded_html(
    right: &super::super::right::RightPane,
    node_path: &crate::system::FileNodePath,
    file_path: &str,
    load: HtmlPreviewLoad,
    disk_signature: super::DiskSignature,
    writable: bool,
    comparison: Option<&FileComparison>,
    local_path: Option<&Path>,
    selection: Option<(usize, usize)>,
) {
    right.show_editor(
        node_path,
        file_path,
        &load.text,
        disk_signature,
        writable,
        comparison,
        Vec::new(),
        load.spellcheck_issues,
    );
    right.file_html_preview.set_html(&load.text, local_path);
    right
        .file_view_split
        .set_end_child(Some(&right.file_html_preview.root));
    if let Some((start, end)) = selection {
        right.file_editor.select_range(start, end);
    }
}

fn base_uri_for_path(path: Option<&Path>) -> Option<String> {
    let base_dir = path.and_then(Path::parent).or(path)?;
    let mut uri = gtk::gio::File::for_path(base_dir).uri().to_string();
    if !uri.ends_with('/') {
        uri.push('/');
    }
    Some(uri)
}
