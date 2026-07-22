use super::dialogs::show_error_dialog;
use super::shortcuts::show_shortcuts_window;
use crate::config::{ConfiguredWorkspace, WorkspaceProvider};
use adw::prelude::*;
use gtk::gio;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN_NAME: &str = "craic";
const DELETED_EXE_SUFFIX: &str = " (deleted)";

pub(super) fn install_actions(app: &adw::Application) {
    let new_window = gio::SimpleAction::new("new_window", None);
    let app_clone = app.clone();
    new_window.connect_activate(move |_, _| {
        if let Err(err) = launch_new_instance() {
            if let Some(app_window) = active_app_window(&app_clone) {
                show_error_dialog(&app_window, "Failed to Open New Window", &err);
            }
        }
    });
    app.add_action(&new_window);
    app.set_accels_for_action("app.new_window", &["<Control>n"]);

    let shortcuts = gio::SimpleAction::new("shortcuts", None);
    let app_clone = app.clone();
    shortcuts.connect_activate(move |_, _| {
        if let Some(app_window) = active_app_window(&app_clone) {
            show_shortcuts_window(&app_window);
        }
    });
    app.add_action(&shortcuts);
    app.set_accels_for_action("app.shortcuts", &["<Control>question"]);

    let about = gio::SimpleAction::new("about", None);
    let app_clone = app.clone();
    about.connect_activate(move |_, _| {
        if let Some(app_window) = active_app_window(&app_clone) {
            let about_dialog = adw::AboutDialog::builder()
                .application_name("Craic")
                .application_icon("dev.craic.Craic")
                .developer_name("Soiri Hiroka")
                .developers(vec!["Soiri Hiroka"])
                .version(env!("CARGO_PKG_VERSION"))
                .comments("A sleek Vibe IDE & Agentic Development Environment")
                .website("https://soirihiroka.github.io/craic/")
                .issue_url("https://github.com/soirihiroka/craic/issues")
                .copyright("© 2026 Soiri Hiroka")
                .license_type(gtk::License::Gpl30)
                .build();
            about_dialog.present(Some(&app_window));
        }
    });
    app.add_action(&about);
    app.set_accels_for_action("app.about", &["F1"]);
}

pub(super) fn app_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some("New Window"), Some("app.new_window"));
    menu.append(Some("Preferences"), Some("app.preferences"));
    menu.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
    menu.append(Some("About Craic"), Some("app.about"));
    menu
}

fn active_app_window(app: &adw::Application) -> Option<adw::ApplicationWindow> {
    app.active_window()?
        .downcast::<adw::ApplicationWindow>()
        .ok()
}

fn launch_new_instance() -> Result<(), String> {
    launch_new_instance_with_workspace(None)
}

pub(in crate::ui) fn launch_workspace_in_new_instance(
    workspace: &ConfiguredWorkspace,
) -> Result<(), String> {
    let executable = resolve_new_instance_executable()?;
    let workspace_path = match &workspace.provider {
        WorkspaceProvider::Local => {
            let path = crate::config::expand_config_path_for_ui(&workspace.path)
                .unwrap_or_else(|| PathBuf::from(&workspace.path));
            if !path.exists() {
                return Err(format!("Workspace path does not exist: {}", path.display()));
            }
            path.canonicalize()
                .unwrap_or(path)
                .to_string_lossy()
                .to_string()
        }
        WorkspaceProvider::Ssh { .. } => workspace.path.clone(),
    };
    log::info!(
        "launching new Craic window executable={} provider={} workspace={}",
        executable.display(),
        workspace.provider_id(),
        workspace_path
    );

    let mut command = Command::new(&executable);
    command
        .arg("--workspace-provider")
        .arg(workspace.provider_id())
        .arg("--workspace-path")
        .arg(workspace_path);
    spawn_new_instance(command, &executable)
}

pub(in crate::ui) fn launch_workspace_location_in_new_instance(
    workspace: &ConfiguredWorkspace,
    path: &str,
    line: Option<usize>,
    column: Option<usize>,
) -> Result<(), String> {
    let executable = resolve_new_instance_executable()?;
    log::info!(
        "launching new Craic window executable={} provider={} workspace={} selected_path={} line={line:?} column={column:?}",
        executable.display(),
        workspace.provider_id(),
        workspace.path,
        path
    );

    let mut command = Command::new(&executable);
    command
        .arg("--workspace-provider")
        .arg(workspace.provider_id())
        .arg("--workspace-path")
        .arg(&workspace.path)
        .arg("--open-path")
        .arg(path);
    if let Some(line) = line {
        command.arg("--line").arg(line.to_string());
    }
    if let Some(column) = column {
        command.arg("--column").arg(column.to_string());
    }
    spawn_new_instance(command, &executable)
}

fn launch_new_instance_with_workspace(workspace_path: Option<&Path>) -> Result<(), String> {
    let executable = resolve_new_instance_executable()?;
    if let Some(workspace_path) = workspace_path {
        log::info!(
            "launching new Craic window executable={} workspace={}",
            executable.display(),
            workspace_path.display()
        );
    } else {
        log::info!(
            "launching new Craic window executable={}",
            executable.display()
        );
    }

    let mut command = Command::new(&executable);
    if let Some(workspace_path) = workspace_path {
        command.arg(workspace_path);
    }
    spawn_new_instance(command, &executable)
}

fn spawn_new_instance(mut command: Command, executable: &Path) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| {
            format!(
                "Could not launch a new Craic instance from {}: {err}",
                executable.display()
            )
        })?;

    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

fn resolve_new_instance_executable() -> Result<PathBuf, String> {
    let mut attempted = Vec::new();

    for candidate in new_instance_executable_candidates() {
        if attempted.contains(&candidate) {
            continue;
        }

        if candidate.is_file() {
            return Ok(candidate);
        }

        log::debug!(
            "new window executable candidate unavailable path={}",
            candidate.display()
        );
        attempted.push(candidate);
    }

    let tried = attempted
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "Could not find the Craic executable. Tried: {tried}"
    ))
}

fn new_instance_executable_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    match std::env::current_exe() {
        Ok(executable) => {
            push_candidate(&mut candidates, executable.clone());
            if let Some(restored) = strip_deleted_exe_suffix(&executable) {
                push_candidate(&mut candidates, restored);
            }
        }
        Err(err) => log::warn!("failed to resolve current executable for new window: {err}"),
    }

    if let Some(arg0) = std::env::args_os().next() {
        push_candidate(&mut candidates, PathBuf::from(arg0));
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            push_candidate(&mut candidates, path.join(BIN_NAME));
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        push_candidate(
            &mut candidates,
            PathBuf::from(home).join(".local/bin").join(BIN_NAME),
        );
    }

    candidates
}

fn push_candidate(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !path.as_os_str().is_empty() && !candidates.contains(&path) {
        candidates.push(path);
    }
}

fn strip_deleted_exe_suffix(path: &Path) -> Option<PathBuf> {
    let path = path.as_os_str().to_str()?;
    let restored = path.strip_suffix(DELETED_EXE_SUFFIX)?;
    if restored.is_empty() {
        return None;
    }
    Some(PathBuf::from(restored))
}
