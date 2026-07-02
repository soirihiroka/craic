use super::{PreviewMatchRequest, PreviewRequest};
use std::rc::Rc;

const MAX_FONT_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;

pub(in crate::ui::pages::file) fn show(request: PreviewRequest<'_>) {
    request
        .right
        .show_provider_loading(request.file_path, "font");

    let files = request.files.clone();
    let file_path = request.file_path.to_string();
    let node_path = request.node_path.clone();
    let len = request.info.len_or_zero();
    let apply_file_path = file_path.clone();

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || read_font_bytes(files.as_ref(), &node_path, &file_path, len),
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

pub(in crate::ui::pages::file) fn show_match(request: PreviewMatchRequest<'_>) {
    show(request.into_preview_request());
}

fn read_font_bytes(
    files: &dyn crate::system::capabilities::files::FileAccess,
    node_path: &crate::system::FileNodePath,
    file_path: &str,
    len: u64,
) -> Result<Vec<u8>, String> {
    if len > MAX_FONT_PREVIEW_BYTES {
        return Err(format!("{file_path} is too large to preview."));
    }

    files
        .read_with_info(node_path, Some(MAX_FONT_PREVIEW_BYTES))?
        .into_bytes()
        .map_err(|err| format!("Unable to preview font: {err}"))
}
