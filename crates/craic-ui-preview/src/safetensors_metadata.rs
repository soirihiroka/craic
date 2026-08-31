use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

pub const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;

pub fn read_metadata_header(path: &Path, display_path: &str) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("Unable to open {display_path}: {error}"))?;
    let mut header_size = [0_u8; 8];
    file.read_exact(&mut header_size)
        .map_err(|error| format!("Unable to read {display_path} header: {error}"))?;

    let header_len = u64::from_le_bytes(header_size);
    let total_len = bounded_total_len(header_len, display_path)?;
    let total_len = usize::try_from(total_len).map_err(|_| {
        format!(
            "{display_path} has a header size that is too large to preview ({header_len} bytes)."
        )
    })?;
    let mut bytes = vec![0_u8; total_len];
    bytes[..8].copy_from_slice(&header_size);
    file.read_exact(&mut bytes[8..])
        .map_err(|error| format!("Unable to read {display_path} header: {error}"))?;
    Ok(bytes)
}

pub fn metadata_text_from_bytes(bytes: &[u8], display_path: &str) -> Result<String, String> {
    let header_size: [u8; 8] = bytes
        .get(..8)
        .ok_or_else(|| format!("Unable to read {display_path} header."))?
        .try_into()
        .expect("slice length checked above");
    let header_len = u64::from_le_bytes(header_size);
    let total_len = bounded_total_len(header_len, display_path)?;
    let total_len = usize::try_from(total_len).map_err(|_| {
        format!(
            "{display_path} has a header size that is too large to preview ({header_len} bytes)."
        )
    })?;
    let bytes = bytes.get(8..total_len).ok_or_else(|| {
        format!("Unable to read the complete Safetensors metadata header from {display_path}.")
    })?;

    let header = std::str::from_utf8(bytes).map_err(|error| {
        format!("Unable to parse Safetensors header as UTF-8 from {display_path}: {error}")
    })?;
    let header: Value = serde_json::from_str(header).map_err(|error| {
        format!("Unable to parse Safetensors metadata JSON from {display_path}: {error}")
    })?;
    let Some(metadata) = header
        .get("__metadata__")
        .and_then(|metadata| metadata.as_object())
    else {
        return Ok("No metadata".to_string());
    };
    if metadata.is_empty() {
        return Ok("No metadata".to_string());
    }

    let mut ordered = BTreeMap::new();
    ordered.extend(
        metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    serde_json::to_string_pretty(&ordered).map_err(|error| {
        format!("Unable to format Safetensors metadata as JSON for preview: {error}")
    })
}

fn bounded_total_len(header_len: u64, display_path: &str) -> Result<u64, String> {
    let total_len = header_len
        .checked_add(8)
        .ok_or_else(|| format!("{display_path} has an invalid header size to preview."))?;
    if total_len > MAX_METADATA_BYTES {
        return Err(format!(
            "{display_path} metadata is too large to preview ({total_len} bytes)."
        ));
    }
    Ok(total_len)
}
