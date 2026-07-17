use super::{PreviewMatchRequest, PreviewRequest};
use std::rc::Rc;

struct FolderPreviewLoad {
    file_count: usize,
    folder_count: usize,
}

pub fn show(request: PreviewRequest<'_>) {
    request
        .right
        .show_provider_loading_message(request.file_path, "Loading folder contents...");

    let files = request.files.clone();
    let workspace_root = request.ctx.workspace_ref().root.absolute.clone();
    let file_path = request.file_path.to_string();
    let node_path = request.node_path.clone();
    let apply_file_path = file_path.clone();

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path,
        move || {
            super::super::folder_entry_counts(files.as_ref(), &node_path).map(
                |(file_count, folder_count)| FolderPreviewLoad {
                    file_count,
                    folder_count,
                },
            )
        },
        move |right, result| match result {
            Ok(load) => {
                right.show_folder_info(
                    &workspace_root,
                    &apply_file_path,
                    load.file_count,
                    load.folder_count,
                );
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

pub fn show_match(request: PreviewMatchRequest<'_>) {
    show(request.into_preview_request());
}
