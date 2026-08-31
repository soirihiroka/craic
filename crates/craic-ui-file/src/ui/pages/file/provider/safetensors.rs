use super::{PreviewMatchRequest, PreviewRequest};
use craic_ui_preview::safetensors_metadata::{metadata_text_from_bytes, read_metadata_header};
use std::path::PathBuf;
use std::rc::Rc;

pub fn show(request: PreviewRequest<'_>) {
    show_safetensors(request);
}

pub fn show_match(request: PreviewMatchRequest<'_>) {
    show_safetensors(request.into_preview_request());
}

fn show_safetensors(request: PreviewRequest<'_>) {
    request.right.show_provider_loading(
        request.load_token,
        request.file_path,
        "Safetensors metadata",
    );
    let file_path = request.file_path.to_string();
    let apply_file_path = file_path.clone();
    let local_path = request.local_path.map(PathBuf::from);
    let prefetched_bytes = request.prefetched_bytes.map(ToOwned::to_owned);

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || {
            read_metadata_text(
                local_path.as_deref(),
                prefetched_bytes.as_deref(),
                &file_path,
            )
        },
        move |right, result| match result {
            Ok(text) => right.show_safetensors_metadata(&apply_file_path, &text),
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

fn read_metadata_text(
    local_path: Option<&std::path::Path>,
    prefetched_bytes: Option<&[u8]>,
    file_path: &str,
) -> Result<String, String> {
    if let Some(bytes) = prefetched_bytes {
        return metadata_text_from_bytes(bytes, file_path);
    }
    let local_path = local_path.ok_or_else(|| {
        format!("Safetensors metadata preview is only available for local files: {file_path}")
    })?;

    let bytes = read_metadata_header(local_path, file_path)?;
    metadata_text_from_bytes(&bytes, file_path)
}
