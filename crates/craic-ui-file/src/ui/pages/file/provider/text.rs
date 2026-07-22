use super::{PreviewMatchRequest, PreviewRequest};
use crate::git::FileComparison;
use std::rc::Rc;
use std::sync::mpsc;

struct TextPreviewLoad {
    text: String,
}

struct TextDiagnostics {
    markdown_lint_issues: Vec<crate::markdown_lint::MarkdownLintIssue>,
    spellcheck_issues: Vec<crate::spellcheck::SpellcheckIssue>,
}

pub fn show(request: PreviewRequest<'_>) {
    show_text(request, None);
}

pub fn show_match(request: PreviewMatchRequest<'_>) {
    let selection = Some((request.start, request.end));
    show_text(request.into_preview_request(), selection);
}

fn show_text(request: PreviewRequest<'_>, selection: Option<(usize, usize)>) {
    request
        .right
        .show_editor_loading(request.load_token, request.file_path, "code");

    let files = request.files.clone();
    let file_path = request.file_path.to_string();
    let apply_node_path = request.node_path.clone();
    let git = (request.ctx.system_ref().provider_kind == crate::system::ProviderKind::Local)
        .then(|| request.ctx.git())
        .flatten();
    let prefetched_bytes = request.prefetched_bytes.map(|bytes| bytes.to_vec());
    let apply_file_path = file_path.clone();
    let disk_signature = super::disk_signature(request.info);
    let writable = request.info.capabilities.writable;
    let language = craic_language::language_support_for_id(
        crate::ui::file_type::detect(&file_path, false).language,
    );
    let deferred_right = Rc::clone(&request.right);
    let load_token = request.load_token;

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || {
            super::super::repository_text_from_prefetch(prefetched_bytes, &file_path)
                .map(|text| TextPreviewLoad { text })
        },
        move |right, result| match result {
            Ok(load) => {
                right.show_editor(
                    &apply_node_path,
                    &apply_file_path,
                    &load.text,
                    disk_signature,
                    writable,
                    None,
                    Vec::new(),
                    Vec::new(),
                );
                right.file_view_split.set_end_child(None::<&gtk::Widget>);
                if let Some((start, end)) = selection {
                    right.file_editor.select_range(start, end);
                }
                let revision = right.file_editor.document_revision();
                log::debug!(
                    "text preview content displayed file_path={} bytes={} revision={revision:?}",
                    apply_file_path,
                    load.text.len(),
                );

                let diagnostics_source = load.text;
                let diagnostics_path = apply_file_path.clone();
                let diagnostics_language = language;
                let diagnostics_files = files.clone();
                super::spawn_preview_load(
                    Rc::clone(&deferred_right),
                    load_token,
                    diagnostics_path.clone(),
                    move || {
                        let allowlist = crate::spellcheck::manifest_allowlist_from_texts(&[(
                            &diagnostics_path,
                            diagnostics_source.as_str(),
                        )]);
                        let spellcheck_issues = crate::spellcheck::check_document(
                            diagnostics_language,
                            Some(&diagnostics_path),
                            &diagnostics_source,
                            &allowlist,
                        );
                        let ignored_rules =
                            crate::workspace_config::markdown_lint_ignored_rules_from_file_access(
                                diagnostics_files.as_ref(),
                            );
                        let markdown_lint_issues = super::super::markdown_lint_issues(
                            &diagnostics_path,
                            &diagnostics_source,
                            &ignored_rules,
                        );
                        TextDiagnostics {
                            markdown_lint_issues,
                            spellcheck_issues,
                        }
                    },
                    move |right, diagnostics| {
                        if right.file_editor.document_revision() != revision {
                            log::debug!(
                                "text preview diagnostics ignored reason=document-changed expected={revision:?} actual={:?}",
                                right.file_editor.document_revision()
                            );
                            return;
                        }
                        log::debug!(
                            "text preview diagnostics applied spellcheck={} markdown_lint={} revision={revision:?}",
                            diagnostics.spellcheck_issues.len(),
                            diagnostics.markdown_lint_issues.len(),
                        );
                        right
                            .file_editor
                            .set_spellcheck_issues(diagnostics.spellcheck_issues);
                        right
                            .file_editor
                            .set_markdown_lint_issues(diagnostics.markdown_lint_issues);
                    },
                );

                if let Some(git) = git.clone() {
                    let (sender, receiver) = mpsc::channel();
                    git.comparison(
                        &apply_file_path,
                        Box::new(move |result| {
                            let _ = sender.send(result.ok());
                        }),
                    );
                    super::receive_preview_load(
                        Rc::clone(&deferred_right),
                        load_token,
                        apply_file_path.clone(),
                        receiver,
                        move |right, comparison: Option<FileComparison>| {
                            if right.file_editor.document_revision() != revision {
                                log::debug!(
                                    "text preview comparison ignored reason=document-changed expected={revision:?} actual={:?}",
                                    right.file_editor.document_revision()
                                );
                                return;
                            }
                            log::debug!(
                                "text preview comparison applied rows={} revision={revision:?}",
                                comparison.as_ref().map_or(0, |value| value.rows.len())
                            );
                            right.file_editor.set_file_diff(comparison.as_ref());
                        },
                    );
                }
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}
