use super::{PreviewMatchRequest, PreviewRequest};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;

const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;

pub(in crate::ui::pages::file) fn show(request: PreviewRequest<'_>) {
    show_safetensors(request);
}

pub(in crate::ui::pages::file) fn show_match(request: PreviewMatchRequest<'_>) {
    show_safetensors(request.into_preview_request());
}

fn show_safetensors(request: PreviewRequest<'_>) {
    request
        .right
        .show_provider_loading(request.file_path, "Safetensors metadata");
    let file_path = request.file_path.to_string();
    let apply_file_path = file_path.clone();
    let local_path = request.local_path.map(PathBuf::from);

    super::spawn_preview_load(
        Rc::clone(&request.right),
        request.load_token,
        file_path.clone(),
        move || read_metadata_text(local_path.as_deref(), &file_path),
        move |right, result| match result {
            Ok(text) => right.show_safetensors_metadata(&apply_file_path, &text),
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

fn read_metadata_text(
    local_path: Option<&std::path::Path>,
    file_path: &str,
) -> Result<String, String> {
    let local_path = local_path.ok_or_else(|| {
        format!("Safetensors metadata preview is only available for local files: {file_path}")
    })?;

    let mut file = fs::File::open(local_path)
        .map_err(|error| format!("Unable to open {file_path}: {error}"))?;
    let mut header_size = [0_u8; 8];
    file.read_exact(&mut header_size)
        .map_err(|error| format!("Unable to read {file_path} header: {error}"))?;

    let header_len = u64::from_le_bytes(header_size);
    let total_len = header_len
        .checked_add(8)
        .ok_or_else(|| format!("{file_path} has an invalid header size to preview."))?;
    if total_len > MAX_METADATA_BYTES {
        return Err(format!(
            "{file_path} metadata is too large to preview ({} bytes).",
            total_len
        ));
    }

    let header_len = usize::try_from(header_len).map_err(|_| {
        format!("{file_path} has a header size that is too large to preview ({header_len} bytes).")
    })?;
    let mut bytes = vec![0_u8; 8 + header_len];
    bytes[..8].copy_from_slice(&header_size);
    file.read_exact(&mut bytes[8..])
        .map_err(|error| format!("Unable to read {file_path} header: {error}"))?;

    let header = std::str::from_utf8(&bytes[8..]).map_err(|error| {
        format!("Unable to parse Safetensors header as UTF-8 from {file_path}: {error}")
    })?;
    let header: serde_json::Value = serde_json::from_str(header).map_err(|error| {
        format!("Unable to parse Safetensors metadata JSON from {file_path}: {error}")
    })?;
    let Some(metadata) = header
        .get("__metadata__")
        .and_then(|metadata| metadata.as_object())
    else {
        return Ok("No metadata".to_string());
    };

    let mut ordered: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    ordered.extend(
        metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    if metadata.is_empty() {
        return Ok("No metadata".to_string());
    }

    serde_json::to_string_pretty(&ordered).map_err(|error| {
        format!("Unable to format Safetensors metadata as JSON for preview: {error}")
    })
}
