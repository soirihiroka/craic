use super::{PreviewMatchRequest, PreviewRequest};
use crate::git::FileComparison;
use std::rc::Rc;

struct TextPreviewLoad {
    text: String,
    comparison: Option<FileComparison>,
    spellcheck_issues: Vec<crate::spellcheck::SpellcheckIssue>,
}

pub(in crate::ui::pages::code) fn show(request: PreviewRequest<'_>) {
    show_text(request, None);
}

pub(in crate::ui::pages::code) fn show_match(request: PreviewMatchRequest<'_>) {
    let selection = Some((request.start, request.end));
    show_text(request.into_preview_request(), selection);
}

fn show_text(request: PreviewRequest<'_>, selection: Option<(usize, usize)>) {
    request.right.show_editor_loading(request.file_path, "code");

    let files = request.files.clone();
    let file_path = request.file_path.to_string();
    let workspace_path = request.workspace_path.clone();
    let git = (request.ctx.system_ref().provider_kind == crate::system::ProviderKind::Local)
        .then(|| request.ctx.git())
        .flatten();
    let prefetched_bytes = request.prefetched_bytes.map(|bytes| bytes.to_vec());
    let apply_file_path = file_path.clone();
    let disk_signature = super::disk_signature(request.metadata);
    let workspace = request.ctx.workspace_ref();
    let language = crate::ui::content::code_editor::language_hint_from_path(&file_path);

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || {
            super::super::read_repository_file_from_prefetch(
                prefetched_bytes,
                files.as_ref(),
                &workspace_path,
            )
            .map(|text| {
                let comparison = git.as_ref().and_then(|git| git.comparison(&file_path).ok());
                let allowlist =
                    crate::spellcheck::load_manifest_allowlist(&workspace, files.clone());
                let spellcheck_issues = crate::spellcheck::check_document(
                    &language,
                    Some(&file_path),
                    &text,
                    &allowlist,
                );
                TextPreviewLoad {
                    text,
                    comparison,
                    spellcheck_issues,
                }
            })
        },
        move |right, result| match result {
            Ok(load) => {
                right.show_editor(
                    &apply_file_path,
                    &load.text,
                    disk_signature,
                    load.comparison.as_ref(),
                    load.spellcheck_issues,
                );
                right.file_view_split.set_end_child(None::<&gtk::Widget>);
                if let Some((start, end)) = selection {
                    right.file_editor.select_range(start, end);
                }
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}
