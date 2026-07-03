use crate::git::GitRepoHandle;
use crate::system::capabilities::files::FileAccess;
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
        git_handle: Option<Arc<GitRepoHandle>>,
    ) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        let file_browser = file_access.map(|file_access| FileBrowser::new(file_access, git_handle));
        if let Some(file_browser) = &file_browser {
            root.append(&file_browser.root);
        }
        Self { root, file_browser }
    }
}
