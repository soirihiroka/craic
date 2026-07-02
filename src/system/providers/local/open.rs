use crate::system::capabilities::open::{
    DesktopOpenAccess, DesktopOpenActivation, DesktopOpenTargetKind,
};
use crate::system::path::{FileNodePath, WorkspacePath, WorkspaceRef};
use gtk::gio;
use gtk::gio::prelude::AppLaunchContextExt;
use gtk::glib::variant::ToVariant;
use gtk::prelude::*;
use std::path::PathBuf;

const FILE_MANAGER_DBUS_NAME: &str = "org.freedesktop.FileManager1";
const FILE_MANAGER_DBUS_PATH: &str = "/org/freedesktop/FileManager1";
const FILE_MANAGER_DBUS_INTERFACE: &str = "org.freedesktop.FileManager1";

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

    fn app_launch_context(
        &self,
        activation: DesktopOpenActivation,
    ) -> Result<gtk::gdk::AppLaunchContext, String> {
        let display = gtk::gdk::Display::default()
            .ok_or_else(|| "GTK display is unavailable for desktop opening.".to_string())?;
        let context = display.app_launch_context();
        context.set_timestamp(if activation.event_time == 0 {
            gtk::gdk::CURRENT_TIME
        } else {
            activation.event_time
        });
        Ok(context)
    }

    fn startup_id_for_file(
        &self,
        context: &gtk::gdk::AppLaunchContext,
        file: &gio::File,
    ) -> String {
        let files = [file.clone()];
        let app = gio::AppInfo::default_for_uri_scheme("file");
        match app.as_ref() {
            Some(app) => context.startup_notify_id(Some(app), &files),
            None => context.startup_notify_id(None::<&gio::AppInfo>, &files),
        }
        .map(|id| id.to_string())
        .unwrap_or_default()
    }

    fn show_with_file_manager(
        &self,
        method: &str,
        local_path: &PathBuf,
        activation: DesktopOpenActivation,
    ) -> Result<(), String> {
        // Wayland compositors require an activation/startup token to focus an
        // existing file-manager window. Generic opener fallbacks and an empty
        // FileManager1 startup id may open the folder but often cannot activate
        // the current window or select the requested item.
        let context = self.app_launch_context(activation)?;
        let file = gio::File::for_path(local_path);
        let startup_id = self.startup_id_for_file(&context, &file);
        let uris = vec![file.uri().to_string()];
        let parameters = (uris.as_slice(), startup_id.as_str()).to_variant();
        let proxy = gio::DBusProxy::for_bus_sync(
            gio::BusType::Session,
            gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES
                | gio::DBusProxyFlags::DO_NOT_CONNECT_SIGNALS,
            None,
            FILE_MANAGER_DBUS_NAME,
            FILE_MANAGER_DBUS_PATH,
            FILE_MANAGER_DBUS_INTERFACE,
            None::<&gio::Cancellable>,
        )
        .map_err(|err| format!("Unable to connect to file manager over DBus: {err}"))?;
        proxy
            .call_sync(
                method,
                Some(&parameters),
                gio::DBusCallFlags::NONE,
                5000,
                None::<&gio::Cancellable>,
            )
            .map_err(|err| format!("Unable to reveal path through file manager DBus: {err}"))?;
        Ok(())
    }

    fn launch_uri(
        &self,
        uri: &str,
        activation: DesktopOpenActivation,
        error: impl FnOnce(gtk::glib::Error) -> String,
    ) -> Result<(), String> {
        let context = self.app_launch_context(activation)?;
        gio::AppInfo::launch_default_for_uri(uri, Some(&context)).map_err(error)
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
            self.show_with_file_manager("ShowFolders", &local_path, activation)?;
            return Ok("Opened path.".to_string());
        }

        let uri = gio::File::for_path(&local_path).uri();
        self.launch_uri(&uri, activation, |err| {
            format!("Failed to open {}: {err}", local_path.display())
        })?;
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
        self.show_with_file_manager("ShowItems", &local_path, activation)?;
        Ok("Opened containing folder.".to_string())
    }
}
