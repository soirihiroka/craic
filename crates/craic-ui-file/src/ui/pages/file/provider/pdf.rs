use super::{PreviewMatchRequest, PreviewRequest};
use crate::system::capabilities::files::{FileOperationEvent, FileReadRequest};
use std::sync::mpsc;

const MAX_PDF_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;

pub fn show(request: PreviewRequest<'_>) {
    request
        .right
        .show_provider_loading(request.load_token, request.file_path, "PDF");

    let file_path = request.file_path.to_string();
    let node_path = request.node_path.clone();
    let len = request.info.len_or_zero();
    let apply_file_path = file_path.clone();
    let (sender, receiver) = mpsc::channel();

    if len > MAX_PDF_PREVIEW_BYTES {
        let _ = sender.send(Err(format!("{file_path} is too large to preview.")));
    } else {
        request.files.read_with_info(
            FileReadRequest {
                path: node_path,
                max_bytes: Some(MAX_PDF_PREVIEW_BYTES),
                cancel_requested: None,
            },
            Box::new(move |event| {
                if let FileOperationEvent::Finished(result) = event {
                    let result = result
                        .map_err(|err| format!("Unable to preview PDF: {err}"))
                        .and_then(|read| {
                            read.into_bytes()
                                .map_err(|err| format!("Unable to preview PDF: {err}"))
                        });
                    let _ = sender.send(result);
                }
            }),
        );
    }

    super::receive_preview_load(
        request.right,
        request.load_token,
        file_path.clone(),
        receiver,
        move |right, result| match result {
            Ok(bytes) => {
                right
                    .file_pdf_preview
                    .set_pdf_single(&apply_file_path, &bytes);
                right.show_pdf_preview(&apply_file_path);
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

pub fn show_match(request: PreviewMatchRequest<'_>) {
    show(request.into_preview_request());
}
