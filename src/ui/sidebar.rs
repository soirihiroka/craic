use super::{picker, widgets};
use crate::git::{GitRepoHandle, RepositorySnapshot};
use crate::system::SystemRef;
use crate::system::capabilities::github::GitHubAccess;
use crate::ui::pages::PageRef;
use adw::prelude::*;
use gtk::gio;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

mod changes;
pub(super) mod changes_panel;
pub(super) mod commit_panel;
pub(super) mod file_browser;
pub(super) mod history;
pub(super) mod mode_switcher;
mod repo_cache;
mod repositories;

pub struct SidebarPane {
    pub root: adw::ToolbarView,
    pub repository_picker: picker::Picker,
    pub(super) mode_switcher: mode_switcher::ModeSwitcher,
    header: adw::HeaderBar,
    page_slot: gtk::Box,
    repo_loading: Rc<Cell<bool>>,
    repo_icon_loading: Rc<Cell<bool>>,
    repo_metadata_loading: Rc<Cell<bool>>,
}

pub fn build(
    menu: &gio::Menu,
    snapshot: Option<&RepositorySnapshot>,
    workspace_key: &str,
    workspace_name: &str,
    system: &SystemRef,
    pages: &[PageRef],
) -> SidebarPane {
    let menu_button = widgets::app_menu_button(menu, true);
    let repository_picker = picker::Picker::new(
        "Search workspaces",
        "Open workspace",
        snapshot
            .map(|snapshot| snapshot.name.as_str())
            .unwrap_or(workspace_name),
        "folder-symbolic",
        "Workspace",
        Vec::new(),
    );

    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .show_start_title_buttons(true)
        .title_widget(&widgets::blank_title())
        .build();
    header.pack_start(&repository_picker.button);
    header.pack_end(&menu_button);

    let mode_switcher = mode_switcher::ModeSwitcher::new(pages);
    let page_slot = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .build();

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .build();
    body.append(&mode_switcher.root);
    body.append(&page_slot);

    let root = adw::ToolbarView::new();
    root.add_top_bar(&header);
    root.set_content(Some(&body));

    let pane = SidebarPane {
        root,
        repository_picker,
        mode_switcher,
        header,
        page_slot,
        repo_loading: Rc::new(Cell::new(false)),
        repo_icon_loading: Rc::new(Cell::new(false)),
        repo_metadata_loading: Rc::new(Cell::new(false)),
    };

    if let Some(snapshot) = snapshot {
        pane.update(snapshot, workspace_key, system);
    }

    pane
}

impl SidebarPane {
    pub(super) fn page_slot(&self) -> gtk::Box {
        self.page_slot.clone()
    }

    pub(super) fn set_workspace_color_active(&self, active: bool) {
        if active {
            self.header.add_css_class("workspace-titlebar-color");
        } else {
            self.header.remove_css_class("workspace-titlebar-color");
        }
    }

    pub fn load_repos_async(&self) {
        repositories::load_repos_async(
            self.repository_picker.clone(),
            self.repo_loading.clone(),
            self.repo_metadata_loading.clone(),
        );
    }

    pub fn refresh_workspace_repo_metadata(
        &self,
        workspace_key: String,
        item_id: Option<String>,
        workspace_host: Option<String>,
        git_handle: Arc<GitRepoHandle>,
        github_access: Option<Arc<dyn GitHubAccess>>,
    ) {
        repositories::refresh_repo_icon_kind(
            workspace_key,
            item_id,
            workspace_host,
            &self.repository_picker,
            self.repo_icon_loading.clone(),
            git_handle,
            github_access,
        );
    }

    pub fn update(&self, snapshot: &RepositorySnapshot, workspace_key: &str, system: &SystemRef) {
        self.repository_picker
            .set_button_label(&repositories::repository_button_label(snapshot, system));
        if let Some(kind) =
            repositories::current_repo_icon_kind(workspace_key, &self.repository_picker)
        {
            self.repository_picker.set_button_icon(kind.icon_name());
        } else {
            self.repository_picker.set_button_spinner();
        }
    }

    pub fn update_page_badges(&self, pages: &[PageRef]) {
        self.mode_switcher.update_badges(pages);
    }

    pub fn set_page_refreshing(&self, index: usize, refreshing: bool) {
        self.mode_switcher.set_refreshing(index, refreshing);
    }

    pub fn set_error(&self, message: &str, workspace_name: &str) {
        log::debug!(
            "workspace picker fallback to label={} reason={}",
            workspace_name,
            message
        );
        self.repository_picker.set_button_label(workspace_name);
        self.repository_picker.set_button_icon("folder-symbolic");
        self.mode_switcher.clear_badges();
    }
}
