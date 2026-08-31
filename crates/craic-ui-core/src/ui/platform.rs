use craic_platform::{OpenPathKind, OpenPathRequest, UiEffect};
use gtk::gio::prelude::{AppLaunchContextExt, DBusProxyExt};
use gtk::glib::variant::ToVariant;
use gtk::prelude::{DisplayExt, GdkAppLaunchContextExt, IsA, WidgetExt};
use gtk::{gio, glib};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn execute_effect(
    effect: UiEffect,
    parent: Option<&impl IsA<gtk::Window>>,
) -> Result<String, String> {
    match effect {
        UiEffect::OpenPath(request) => open_path(request, parent),
        UiEffect::RevealPath(path) => {
            reveal_path(&path, parent);
            Ok("Opened containing folder.".to_string())
        }
        UiEffect::OpenUrl(url) => {
            let display = gtk::gdk::Display::default()
                .ok_or_else(|| "GTK display is unavailable for URL opening.".to_string())?;
            let context = display.app_launch_context();
            context.set_timestamp(gtk::gdk::CURRENT_TIME);
            gio::AppInfo::launch_default_for_uri(&url, Some(&context))
                .map_err(|error| format!("Failed to open URL: {error}"))?;
            Ok("Opened URL.".to_string())
        }
        _ => Err("This UI effect requires a dialog handler.".to_string()),
    }
}

fn open_path(
    request: OpenPathRequest,
    parent: Option<&impl IsA<gtk::Window>>,
) -> Result<String, String> {
    log::info!("GTK open path start path={}", request.path.display());
    if request.kind == OpenPathKind::File {
        launch_path(&request.path, parent);
        return Ok("Opened path.".to_string());
    }

    let metadata = request
        .path
        .metadata()
        .map_err(|error| format!("Unable to inspect {}: {error}", request.path.display()))?;
    if metadata.is_dir() {
        launch_path(&request.path, parent);
    } else {
        reveal_path(&request.path, parent);
    }
    Ok("Opened path.".to_string())
}

fn launch_path(path: &Path, parent: Option<&impl IsA<gtk::Window>>) {
    let path_display = path.display().to_string();
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    launcher.launch(
        parent,
        None::<&gio::Cancellable>,
        move |result| match result {
            Ok(()) => log::info!("GTK file launcher complete path={path_display}"),
            Err(error) => log::warn!("GTK file launcher failed path={path_display}: {error}"),
        },
    );
}

fn launch_containing_folder(path: &Path, parent: Option<&impl IsA<gtk::Window>>) {
    let path_display = path.display().to_string();
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    launcher.open_containing_folder(
        parent,
        None::<&gio::Cancellable>,
        move |result| match result {
            Ok(()) => log::info!("GTK file reveal complete path={path_display}"),
            Err(error) => log::warn!("GTK file reveal failed path={path_display}: {error}"),
        },
    );
}

fn reveal_path(path: &Path, parent: Option<&impl IsA<gtk::Window>>) {
    let Ok(file) = File::open(path) else {
        launch_containing_folder(path, parent);
        return;
    };

    // On Wayland, reveal first through the portal and activate the parent folder second.
    // Reversing this ordering can select the file without focusing its existing window.
    let path = path.to_path_buf();
    let path_display = path.display().to_string();
    let fd_list = gio::UnixFDList::from_array([file]);
    let options = glib::VariantDict::default();
    if let Some(parent) = parent {
        let parent: &gtk::Window = parent.as_ref();
        let context = parent.display().app_launch_context();
        context.set_timestamp(gtk::gdk::CURRENT_TIME);
        let files = [gio::File::for_path(&path)];
        if let Some(token) = context.startup_notify_id(gio::AppInfo::NONE, &files) {
            options.insert("activation_token", token.as_str());
        }
    }
    let parameters = ("", glib::variant::Handle::from(0), options).to_variant();
    let parent = parent.map(|parent| parent.as_ref().clone());
    let fallback_parent = parent.clone();
    let fallback_path = path.clone();
    let focus_parent = parent.clone();
    let focus_path = path.clone();

    gio::DBusProxy::for_bus(
        gio::BusType::Session,
        gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES | gio::DBusProxyFlags::DO_NOT_CONNECT_SIGNALS,
        None::<&gio::DBusInterfaceInfo>,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.OpenURI",
        None::<&gio::Cancellable>,
        move |proxy| match proxy {
            Ok(proxy) => {
                let path_display = path_display.clone();
                proxy.call_with_unix_fd_list(
                    "OpenDirectory",
                    Some(&parameters),
                    gio::DBusCallFlags::NONE,
                    -1,
                    Some(&fd_list),
                    None::<&gio::Cancellable>,
                    move |result| match result {
                        Ok(_) => {
                            log::info!("GTK file reveal portal complete path={path_display}");
                            activate_parent_folder_after_reveal(&focus_path, focus_parent);
                        }
                        Err(error) => {
                            log::warn!(
                                "GTK file reveal portal failed path={path_display}: {error}"
                            );
                            launch_containing_folder(&fallback_path, fallback_parent.as_ref());
                        }
                    },
                );
            }
            Err(error) => {
                log::warn!("GTK file reveal portal proxy failed path={path_display}: {error}");
                launch_containing_folder(&path, parent.as_ref());
            }
        },
    );
}

fn activate_parent_folder_after_reveal(path: &Path, parent: Option<gtk::Window>) {
    let Some(parent_directory) = path.parent().map(PathBuf::from) else {
        return;
    };
    let path_display = path.display().to_string();
    glib::timeout_add_local_once(Duration::from_millis(120), move || {
        log::debug!("GTK file reveal activating parent folder path={path_display}");
        launch_path(&parent_directory, parent.as_ref());
    });
}
