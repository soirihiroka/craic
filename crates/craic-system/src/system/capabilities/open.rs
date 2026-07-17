use crate::system::path::FileNodePath;
use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopOpenTargetKind {
    File,
    Folder,
}

#[derive(Clone, Default)]
pub struct DesktopOpenActivation {
    pub parent: Option<gtk::Window>,
}

impl DesktopOpenActivation {
    pub fn from_parent(parent: Option<&impl IsA<gtk::Window>>) -> Self {
        Self {
            parent: parent.map(|parent| parent.as_ref().clone()),
        }
    }
}

pub trait DesktopOpenAccess: Send + Sync {
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
