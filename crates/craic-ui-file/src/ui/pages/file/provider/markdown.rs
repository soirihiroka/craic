use super::{PreviewMatchRequest, PreviewRequest};
use crate::git;
use craic_ui_preview::markdown_preview::MarkdownPreviewDocument;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;

struct MarkdownPreviewLoad {
    text: String,
    document: MarkdownPreviewDocument,
    markdown_lint_issues: Vec<crate::markdown_lint::MarkdownLintIssue>,
    spellcheck_issues: Vec<crate::spellcheck::SpellcheckIssue>,
}

#[derive(Clone, Copy)]
enum MarkupKind {
    Markdown,
    Rst,
}

pub fn show(request: PreviewRequest<'_>) {
    show_markup(request, None, MarkupKind::Markdown);
}

pub fn show_match(request: PreviewMatchRequest<'_>) {
    let selection = Some((request.start, request.end));
    show_markup(
        request.into_preview_request(),
        selection,
        MarkupKind::Markdown,
    );
}

pub fn show_rst(request: PreviewRequest<'_>) {
    show_markup(request, None, MarkupKind::Rst);
}

pub fn show_rst_match(request: PreviewMatchRequest<'_>) {
    let selection = Some((request.start, request.end));
    show_markup(request.into_preview_request(), selection, MarkupKind::Rst);
}

fn show_markup(
    request: PreviewRequest<'_>,
    selection: Option<(usize, usize)>,
    markup_kind: MarkupKind,
) {
    let display_kind = match markup_kind {
        MarkupKind::Markdown => "Markdown",
        MarkupKind::Rst => "reStructuredText",
    };
    request
        .right
        .show_editor_loading(request.load_token, request.file_path, display_kind);

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
    let language = craic_language::language_support_for_id(
        crate::ui::file_type::detect(&file_path, false).language,
    );
    let comparison_right = Rc::clone(&request.right);
    let comparison_token = request.load_token;

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || {
            super::super::repository_text_from_prefetch(prefetched_bytes, &file_path).and_then(
                |text| {
                    let allowlist = crate::spellcheck::manifest_allowlist_from_texts(&[(
                        &file_path,
                        text.as_str(),
                    )]);
                    let spellcheck_issues = crate::spellcheck::check_document(
                        language,
                        Some(&file_path),
                        &text,
                        &allowlist,
                    );
                    let ignored_rules =
                        crate::workspace_config::markdown_lint_ignored_rules_from_file_access(
                            files.as_ref(),
                        );
                    let markdown_lint_issues = crate::markdown_lint::check_language_document(
                        language,
                        Some(&file_path),
                        &text,
                        &ignored_rules,
                    );
                    let document = match markup_kind {
                        MarkupKind::Markdown => MarkdownPreviewDocument::parse(&text),
                        MarkupKind::Rst => MarkdownPreviewDocument::parse_rst(&text),
                    };
                    Ok(MarkdownPreviewLoad {
                        text,
                        document,
                        markdown_lint_issues,
                        spellcheck_issues,
                    })
                },
            )
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
                    let preview_base_path = local_path.clone();
                    let mut load = Some(load);
                    super::receive_preview_load(
                        Rc::clone(&comparison_right),
                        comparison_token,
                        apply_file_path.clone(),
                        receiver,
                        move |right, comparison: Option<git::FileComparison>| {
                            let Some(load) = load.take() else {
                                return;
                            };
                            right.show_editor(
                                &node_path,
                                &file_path,
                                &load.text,
                                disk_signature,
                                writable,
                                comparison.as_ref(),
                                load.markdown_lint_issues,
                                load.spellcheck_issues,
                            );
                            if load.text.trim().is_empty() {
                                right
                                    .file_view_split
                                    .set_end_child(Some(&right.file_markdown_status));
                            } else {
                                right.file_markdown_preview.set_document_with_base_path(
                                    load.document,
                                    preview_base_path.as_deref(),
                                );
                                let _ = right.file_markdown_preview.scroll_to_source_offset(
                                    right.file_editor.source_offset_at_scroll_top(),
                                );
                                right
                                    .file_view_split
                                    .set_end_child(Some(&right.file_markdown_preview.root));
                            }
                            if let Some((start, end)) = selection {
                                right.file_editor.select_range(start, end);
                            }
                        },
                    );
                } else {
                    right.show_editor(
                        &apply_node_path,
                        &apply_file_path,
                        &load.text,
                        disk_signature,
                        writable,
                        None,
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
                        let _ = right.file_markdown_preview.scroll_to_source_offset(
                            right.file_editor.source_offset_at_scroll_top(),
                        );
                        right
                            .file_view_split
                            .set_end_child(Some(&right.file_markdown_preview.root));
                    }
                    if let Some((start, end)) = selection {
                        right.file_editor.select_range(start, end);
                    }
                }
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}
