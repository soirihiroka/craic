use super::path::FileNodePath;
use crate::system::capabilities::files::{
    FileAccess, FileNodeInfo, FileOperation, FileReadRequest, wait_file_operation,
};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
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

pub fn materialize_for_view(
    files: Arc<dyn FileAccess>,
    source: FileNodeInfo,
    max_bytes: Option<u64>,
) -> mpsc::Receiver<Result<MaterializedFile, String>> {
    let path = source.path.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let events = files.read_with_info_events(FileReadRequest {
        path,
        max_bytes,
        cancel_requested: None,
    });
    thread::spawn(move || {
        let result = wait_file_operation(events, FileOperation::Read)
            .map_err(|err| err.to_string())
            .and_then(|read| read.into_bytes())
            .and_then(|bytes| materialize_bytes_for_view(&source, bytes, max_bytes));
        let _ = sender.send(result);
    });
    receiver
}

pub fn materialize_bytes_for_view(
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
