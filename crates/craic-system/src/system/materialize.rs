use super::path::FileNodePath;
use crate::system::capabilities::files::{
    FileAccess, FileNodeInfo, FileOperationEvent, FileReadRequest,
};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct MaterializedFile {
    pub source: FileNodePath,
    pub local_path: PathBuf,
    pub len: u64,
    pub created_at: SystemTime,
}

impl MaterializedFile {
    pub fn new(source: FileNodePath, local_path: PathBuf, len: u64) -> Self {
        log::debug!(
            "materialized file source={} local_path={} len={}",
            source.display(),
            local_path.display(),
            len
        );
        Self {
            source,
            local_path,
            len,
            created_at: SystemTime::now(),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.local_path
    }
}

impl Drop for MaterializedFile {
    fn drop(&mut self) {
        match fs::remove_file(&self.local_path) {
            Ok(()) => log::debug!(
                "materialized file removed source={} local_path={}",
                self.source.display(),
                self.local_path.display()
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => log::warn!(
                "materialized file cleanup failed source={} local_path={} error={}",
                self.source.display(),
                self.local_path.display(),
                err
            ),
        }
    }
}

pub fn materialize_for_view<F>(
    files: Arc<dyn FileAccess>,
    source: FileNodeInfo,
    max_bytes: Option<u64>,
    callback: F,
) where
    F: FnOnce(Result<MaterializedFile, String>) + Send + 'static,
{
    let path = source.path.clone();
    let callback = Arc::new(Mutex::new(Some(callback)));
    files.read_with_info(
        FileReadRequest {
            path,
            max_bytes,
            cancel_requested: None,
        },
        Box::new(move |event| {
            if let FileOperationEvent::Finished(result) = event {
                let result = result
                    .map_err(|err| err.to_string())
                    .and_then(|read| read.into_bytes())
                    .and_then(|bytes| materialize_bytes(&source, bytes, max_bytes));
                let callback = callback
                    .lock()
                    .ok()
                    .and_then(|mut callback| callback.take());
                if let Some(callback) = callback {
                    callback(result);
                }
            }
        }),
    );
}

fn materialize_bytes(
    source: &FileNodeInfo,
    bytes: Vec<u8>,
    max_bytes: Option<u64>,
) -> Result<MaterializedFile, String> {
    if let Some(max_bytes) = max_bytes
        && bytes.len() as u64 > max_bytes
    {
        return Err(format!(
            "{} is too large to materialize for preview.",
            source.path.display()
        ));
    }

    let suffix = source
        .path
        .file_name()
        .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    let local_path =
        std::env::temp_dir().join(format!("craic-preview-{}{}", Uuid::new_v4(), suffix));
    let mut file = fs::File::create(&local_path)
        .map_err(|err| format!("Failed to create preview materialization: {err}"))?;
    file.write_all(&bytes)
        .map_err(|err| format!("Failed to write preview materialization: {err}"))?;
    log::info!(
        "materialized preview source={} local_path={} bytes={}",
        source.path.display(),
        local_path.display(),
        bytes.len()
    );
    Ok(MaterializedFile::new(
        source.path.clone(),
        local_path,
        bytes.len() as u64,
    ))
}
