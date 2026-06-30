use super::{PreviewMatchRequest, PreviewRequest};
use std::rc::Rc;

const MAX_FONT_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;

pub(in crate::ui::pages::code) fn show(request: PreviewRequest<'_>) {
    request
        .right
        .show_provider_loading(request.file_path, "font");

    let files = request.files.clone();
    let file_path = request.file_path.to_string();
    let workspace_path = request.workspace_path.clone();
    let len = request.metadata.len;
    let apply_file_path = file_path.clone();

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || read_font_bytes(files.as_ref(), &workspace_path, &file_path, len),
        move |right, result| match result {
            Ok(bytes) => {
                right
                    .file_font_preview
                    .set_font_single(&apply_file_path, &bytes);
                right.show_font_preview(&apply_file_path);
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

pub(in crate::ui::pages::code) fn show_match(request: PreviewMatchRequest<'_>) {
    show(request.into_preview_request());
}

fn read_font_bytes(
    files: &dyn crate::system::capabilities::files::FileAccess,
    workspace_path: &crate::system::WorkspacePath,
    file_path: &str,
    len: u64,
) -> Result<Vec<u8>, String> {
    if len > MAX_FONT_PREVIEW_BYTES {
        return Err(format!("{file_path} is too large to preview."));
    }

    files
        .read_with_metadata(workspace_path, Some(MAX_FONT_PREVIEW_BYTES))?
        .into_bytes()
        .map_err(|err| format!("Unable to preview font: {err}"))
}
