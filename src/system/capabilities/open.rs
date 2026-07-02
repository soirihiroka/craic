use crate::system::path::FileNodePath;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopOpenTargetKind {
    File,
    Folder,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DesktopOpenActivation {
    pub(crate) event_time: u32,
}

impl DesktopOpenActivation {
    pub(crate) fn from_event_time(event_time: u32) -> Self {
        Self { event_time }
    }
}

pub(crate) trait DesktopOpenAccess: Send + Sync {
    fn open_path(
        &self,
        path: &FileNodePath,
        kind: DesktopOpenTargetKind,
        activation: DesktopOpenActivation,
    ) -> Result<String, String>;
    fn reveal_path(
        &self,
        path: &FileNodePath,
        activation: DesktopOpenActivation,
    ) -> Result<String, String>;
    fn open_url(&self, url: &str, activation: DesktopOpenActivation) -> Result<String, String>;
}
