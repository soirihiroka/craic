use super::{PreviewMatchRequest, PreviewRequest};
use crate::git;
use crate::ui::components::markdown_preview::MarkdownPreviewDocument;
use std::path::Path;
use std::rc::Rc;

struct MarkdownPreviewLoad {
    text: String,
    document: MarkdownPreviewDocument,
    comparison: Option<git::FileComparison>,
    markdown_lint_issues: Vec<crate::markdown_lint::MarkdownLintIssue>,
    spellcheck_issues: Vec<crate::spellcheck::SpellcheckIssue>,
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
