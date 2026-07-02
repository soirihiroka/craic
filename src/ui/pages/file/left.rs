use crate::system::capabilities::{files::FileAccess, git::GitAccess};
use crate::ui::sidebar::file_browser::FileBrowser;
use adw::prelude::*;
use std::rc::Rc;
use std::sync::Arc;

pub(super) struct LeftPane {
    pub(super) root: gtk::Box,
    pub(super) file_browser: Option<Rc<FileBrowser>>,
}

impl LeftPane {
    pub(super) fn new(
        file_access: Option<Arc<dyn FileAccess>>,
        git_access: Option<Arc<dyn GitAccess>>,
    ) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        let file_browser = file_access.map(|file_access| FileBrowser::new(file_access, git_access));
        if let Some(file_browser) = &file_browser {
            root.append(&file_browser.root);
        }
        Self { root, file_browser }
    }
}
