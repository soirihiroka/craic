use super::path::SystemPath;
use crate::system::capabilities::files::FileAccess;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct MaterializedFile {
    pub(crate) source: SystemPath,
    pub(crate) local_path: PathBuf,
    pub(crate) len: u64,
    pub(crate) created_at: SystemTime,
}

impl MaterializedFile {
    pub(crate) fn new(source: SystemPath, local_path: PathBuf, len: u64) -> Self {
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

    pub(crate) fn path(&self) -> &std::path::Path {
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

pub(crate) fn materialize_for_view(
    files: &dyn FileAccess,
    source: &SystemPath,
    max_bytes: u64,
) -> Result<MaterializedFile, String> {
    let bytes = files
        .read_with_metadata(&source.path, Some(max_bytes))?
        .into_bytes()?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{} is too large to materialize for preview.",
            source.display()
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
        source.display(),
        local_path.display(),
        bytes.len()
    );
    Ok(MaterializedFile::new(
        source.clone(),
        local_path,
        bytes.len() as u64,
    ))
}
