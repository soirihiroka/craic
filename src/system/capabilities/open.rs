use crate::system::path::FileNodePath;
use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopOpenTargetKind {
    File,
    Folder,
}

#[derive(Clone, Default)]
pub(crate) struct DesktopOpenActivation {
    pub(crate) parent: Option<gtk::Window>,
}

impl DesktopOpenActivation {
    pub(crate) fn from_parent(parent: Option<&impl IsA<gtk::Window>>) -> Self {
        Self {
            parent: parent.map(|parent| parent.as_ref().clone()),
        }
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
}
