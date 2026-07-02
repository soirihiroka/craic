use crate::system::capabilities::open::{
    DesktopOpenAccess, DesktopOpenActivation, DesktopOpenTargetKind,
};
use crate::system::path::{FileNodePath, WorkspacePath, WorkspaceRef};
use gtk::gio::prelude::{AppLaunchContextExt, DBusProxyExt};
use gtk::glib::variant::ToVariant;
use gtk::prelude::{DisplayExt, GdkAppLaunchContextExt};
use gtk::{gio, glib};
use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug)]
pub(crate) struct LocalDesktopOpenAccess {
    workspace: WorkspaceRef,
    root: PathBuf,
}

impl LocalDesktopOpenAccess {
    pub(crate) fn new(workspace: WorkspaceRef) -> Self {
        let root = PathBuf::from(&workspace.root.absolute);
        Self { workspace, root }
    }

    fn workspace_path(&self, path: &FileNodePath) -> Result<WorkspacePath, String> {
        path.to_workspace_path(&self.workspace)
            .ok_or_else(|| "Opening virtual or external file nodes is unavailable.".to_string())
    }

    fn local_path_for_workspace(&self, path: &WorkspacePath) -> Result<PathBuf, String> {
        if let Some(relative) = path
            .relative
            .as_deref()
            .filter(|relative| !relative.is_empty())
        {
            let local_path = self.root.join(relative);
            return if local_path.starts_with(&self.root) {
                Ok(local_path)
            } else {
                Err("Path is outside the workspace.".to_string())
            };
        }

        Ok(PathBuf::from(&path.absolute))
    }

    fn local_path(&self, path: &FileNodePath) -> Result<PathBuf, String> {
        let workspace_path = self.workspace_path(path)?;
        self.local_path_for_workspace(&workspace_path)
    }

    fn launch_file(&self, local_path: &PathBuf, activation: DesktopOpenActivation) {
        let path_display = local_path.display().to_string();
        let workspace_name = self.workspace.display_name.clone();
        let file = gio::File::for_path(local_path);
        let launcher = gtk::FileLauncher::new(Some(&file));
        launcher.launch(
            activation.parent.as_ref(),
            None::<&gio::Cancellable>,
            move |result| match result {
                Ok(()) => log::info!(
                    "local file launcher complete workspace={} path={}",
                    workspace_name,
                    path_display
                ),
                Err(err) => log::warn!(
                    "local file launcher failed workspace={} path={}: {}",
                    workspace_name,
                    path_display,
                    err
                ),
            },
        );
    }

    fn portal_activation_token(
        local_path: &PathBuf,
        activation: &DesktopOpenActivation,
    ) -> Option<glib::GString> {
        let parent = activation.parent.as_ref()?;
        let context = gtk::prelude::WidgetExt::display(parent).app_launch_context();
        context.set_timestamp(gtk::gdk::CURRENT_TIME);
        let files = [gio::File::for_path(local_path)];
        context.startup_notify_id(gio::AppInfo::NONE, &files)
    }

    fn launch_containing_folder(&self, local_path: &PathBuf, activation: DesktopOpenActivation) {
        let path_display = local_path.display().to_string();
        let workspace_name = self.workspace.display_name.clone();
        let file = gio::File::for_path(local_path);
        let launcher = gtk::FileLauncher::new(Some(&file));
        launcher.open_containing_folder(
            activation.parent.as_ref(),
            None::<&gio::Cancellable>,
            move |result| match result {
                Ok(()) => log::info!(
                    "local file reveal complete workspace={} path={}",
                    workspace_name,
                    path_display
                ),
                Err(err) => log::warn!(
                    "local file reveal failed workspace={} path={}: {}",
                    workspace_name,
                    path_display,
                    err
                ),
            },
        );
    }

    fn activate_parent_folder_after_reveal(
        &self,
        local_path: &PathBuf,
        activation: DesktopOpenActivation,
    ) {
        let Some(parent_dir) = local_path.parent().map(PathBuf::from) else {
            return;
        };
        let opener = self.clone();
        let path_display = local_path.display().to_string();
        let workspace_name = self.workspace.display_name.clone();
        glib::timeout_add_local_once(Duration::from_millis(120), move || {
            log::debug!(
                "local file reveal activating parent folder workspace={} path={}",
                workspace_name,
                path_display
            );
            opener.launch_file(&parent_dir, activation);
        });
    }

    fn open_containing_folder(&self, local_path: &PathBuf, activation: DesktopOpenActivation) {
        let file = match File::open(local_path) {
            Ok(file) => file,
            Err(err) => {
                log::warn!(
                    "local file reveal fd open failed workspace={} path={}: {}",
                    self.workspace.display_name,
                    local_path.display(),
                    err
                );
                self.launch_containing_folder(local_path, activation);
                return;
            }
        };

        // Maintainer warning: do not simplify this unless you have re-tested
        // both selection and focus on Wayland. "Show in file manager" is not
        // equivalent to opening a path. GtkFileLauncher::open_containing_folder
        // and raw FileManager1.ShowItems can reveal the item but fail to
        // activate an existing file-manager window under Wayland focus rules.
        // xdg-open is also intentionally not used here because it cannot pass
        // portal activation data. The sequence below uses the native
        // xdg-desktop-portal OpenDirectory fd API to select the item, then
        // activates the parent folder through the normal folder-open path that
        // GTK already handles correctly for focus. The order matters: reveal
        // first, focus the folder window second.
        let path_display = local_path.display().to_string();
        let workspace_name = self.workspace.display_name.clone();
        let fd_list = gio::UnixFDList::from_array([file]);
        let options = glib::VariantDict::default();
        if let Some(token) = Self::portal_activation_token(local_path, &activation) {
            options.insert("activation_token", token.as_str());
        } else {
            log::debug!(
                "local file reveal portal activation token unavailable workspace={} path={}",
                workspace_name,
                path_display
            );
        }
        let parameters = ("", glib::variant::Handle::from(0), options).to_variant();
        let fallback = self.clone();
        let fallback_path = local_path.clone();
        let fallback_activation = activation.clone();
        let focus_opener = self.clone();
        let focus_path = local_path.clone();
        let focus_activation = activation.clone();

        gio::DBusProxy::for_bus(
            gio::BusType::Session,
            gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES
                | gio::DBusProxyFlags::DO_NOT_CONNECT_SIGNALS,
            None::<&gio::DBusInterfaceInfo>,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.OpenURI",
            None::<&gio::Cancellable>,
            move |proxy| match proxy {
                Ok(proxy) => {
                    proxy.call_with_unix_fd_list(
                        "OpenDirectory",
                        Some(&parameters),
                        gio::DBusCallFlags::NONE,
                        -1,
                        Some(&fd_list),
                        None::<&gio::Cancellable>,
                        move |result| match result {
                            Ok(_) => {
                                log::info!(
                                    "local file reveal portal complete workspace={} path={}",
                                    workspace_name,
                                    path_display
                                );
                                focus_opener.activate_parent_folder_after_reveal(
                                    &focus_path,
                                    focus_activation,
                                );
                            }
                            Err(err) => {
                                log::warn!(
                                    "local file reveal portal failed workspace={} path={}: {}",
                                    workspace_name,
                                    path_display,
                                    err
                                );
                                fallback
                                    .launch_containing_folder(&fallback_path, fallback_activation);
                            }
                        },
                    );
                }
                Err(err) => {
                    log::warn!(
                        "local file reveal portal proxy failed workspace={} path={}: {}",
                        workspace_name,
                        path_display,
                        err
                    );
                    fallback.launch_containing_folder(&fallback_path, fallback_activation);
                }
            },
        );
    }

    fn open_path_in_file_manager(
        &self,
        local_path: &PathBuf,
        activation: DesktopOpenActivation,
    ) -> Result<(), String> {
        let metadata = local_path
            .metadata()
            .map_err(|err| format!("Unable to inspect {}: {err}", local_path.display()))?;
        if metadata.is_dir() {
            self.launch_file(local_path, activation);
        } else {
            self.open_containing_folder(local_path, activation);
        }
        Ok(())
    }
}

impl DesktopOpenAccess for LocalDesktopOpenAccess {
    fn open_path(
        &self,
        path: &FileNodePath,
        kind: DesktopOpenTargetKind,
        activation: DesktopOpenActivation,
    ) -> Result<String, String> {
        let local_path = self.local_path(path)?;
        log::info!(
            "local open path start workspace={} path={}",
            self.workspace.display_name,
            local_path.display()
        );
        if kind == DesktopOpenTargetKind::Folder {
            self.open_path_in_file_manager(&local_path, activation)?;
            return Ok("Opened path.".to_string());
        }

        self.launch_file(&local_path, activation);
        Ok("Opened path.".to_string())
    }

    fn reveal_path(
        &self,
        path: &FileNodePath,
        activation: DesktopOpenActivation,
    ) -> Result<String, String> {
        let local_path = self.local_path(path)?;
        log::info!(
            "local reveal path start workspace={} path={}",
            self.workspace.display_name,
            local_path.display()
        );
        self.open_containing_folder(&local_path, activation);
        Ok("Opened containing folder.".to_string())
    }
}
