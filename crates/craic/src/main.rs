use adw::prelude::*;
use gtk::{gio, glib};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use craic_config as config;

const APP_ID: &str = "dev.craic.Craic";

fn main() -> glib::ExitCode {
    let launch_start = Instant::now();
    let crash_log_dir = craic_ui::install_crash_log();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("startup process begin app_id={APP_ID}");
    if let Some(crash_log_dir) = crash_log_dir {
        log::info!(
            "crash dump directory initialized path={}",
            crash_log_dir.display()
        );
    }
    let startup_workspace = startup_workspace_arg();
    if let Some(error) = startup_workspace.error.as_deref() {
        log::warn!("startup workspace argument ignored: {error}");
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    log::info!(
        "startup step=create-application step_ms={} total_ms={}",
        launch_start.elapsed().as_millis(),
        launch_start.elapsed().as_millis()
    );
    app.connect_startup(|_| {
        let step_start = Instant::now();
        libpanel::init();
        log::info!(
            "startup step=libpanel-init step_ms={}",
            step_start.elapsed().as_millis()
        );
    });
    app.connect_activate(move |app| {
        craic_ui::build_ui(
            app,
            launch_start,
            startup_workspace.workspace.clone(),
            startup_workspace.open_location.clone(),
            startup_workspace.error.clone(),
        )
    });
    let app_args = application_args_without_workspace();
    let exit_code = app.run_with_args(&app_args);
    log::info!(
        "application exit total_ms={}",
        launch_start.elapsed().as_millis()
    );
    exit_code
}

#[derive(Clone, Default)]
struct StartupWorkspaceArg {
    workspace: Option<config::ConfiguredWorkspace>,
    open_location: Option<craic_ui::StartupOpenLocation>,
    error: Option<String>,
}

fn startup_workspace_arg() -> StartupWorkspaceArg {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        return StartupWorkspaceArg::default();
    }
    if args.len() > 1 {
        if args
            .first()
            .is_some_and(|arg| arg == "--workspace-provider")
        {
            return startup_navigation_args(&args).unwrap_or_else(|error| StartupWorkspaceArg {
                workspace: None,
                open_location: None,
                error: Some(error),
            });
        }
        return StartupWorkspaceArg {
            workspace: None,
            open_location: None,
            error: Some("Expected at most one workspace path.".to_string()),
        };
    }

    match resolve_workspace_arg(Path::new(&args[0])) {
        Ok(path) => {
            log::info!(
                "startup workspace argument resolved path={}",
                path.display()
            );
            StartupWorkspaceArg {
                workspace: Some(config::ConfiguredWorkspace::local(
                    path.to_string_lossy().to_string(),
                )),
                open_location: None,
                error: None,
            }
        }
        Err(error) => StartupWorkspaceArg {
            workspace: None,
            open_location: None,
            error: Some(error),
        },
    }
}

fn startup_navigation_args(args: &[OsString]) -> Result<StartupWorkspaceArg, String> {
    let mut provider = None;
    let mut workspace_path = None;
    let mut open_path = None;
    let mut line = None;
    let mut column = None;
    let mut index = 0;

    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| "Startup option names must be valid UTF-8.".to_string())?;
        index += 1;
        let value = args
            .get(index)
            .ok_or_else(|| format!("Missing value for {flag}."))?;
        index += 1;

        match flag {
            "--workspace-provider" => provider = Some(os_string_value(value, flag)?),
            "--workspace-path" => workspace_path = Some(os_string_value(value, flag)?),
            "--open-path" => open_path = Some(os_string_value(value, flag)?),
            "--line" => line = Some(positive_usize(value, flag)?),
            "--column" => column = Some(positive_usize(value, flag)?),
            _ => return Err(format!("Unknown startup option: {flag}.")),
        }
    }

    let provider = provider.ok_or_else(|| "Missing --workspace-provider.".to_string())?;
    let workspace_path = workspace_path.ok_or_else(|| "Missing --workspace-path.".to_string())?;
    if column.is_some() && line.is_none() {
        return Err("--column requires --line.".to_string());
    }
    if line.is_some() && open_path.is_none() {
        return Err("--line requires --open-path.".to_string());
    }

    let provider = match provider.as_str() {
        "local" => config::WorkspaceProvider::Local,
        value => {
            let host = value
                .strip_prefix("ssh:")
                .filter(|host| !host.is_empty())
                .ok_or_else(|| format!("Unsupported workspace provider: {value}."))?;
            config::WorkspaceProvider::Ssh {
                host: host.to_string(),
            }
        }
    };
    let workspace_path = if provider == config::WorkspaceProvider::Local {
        resolve_workspace_arg(Path::new(&workspace_path))?
            .to_string_lossy()
            .to_string()
    } else {
        workspace_path
    };

    Ok(StartupWorkspaceArg {
        workspace: Some(config::ConfiguredWorkspace {
            path: workspace_path,
            provider,
            display_name: None,
            color: None,
        }),
        open_location: open_path.map(|path| craic_ui::StartupOpenLocation { path, line, column }),
        error: None,
    })
}

fn os_string_value(value: &OsString, flag: &str) -> Result<String, String> {
    value
        .to_str()
        .map(ToString::to_string)
        .ok_or_else(|| format!("{flag} must be valid UTF-8."))
}

fn positive_usize(value: &OsString, flag: &str) -> Result<usize, String> {
    let value = os_string_value(value, flag)?;
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{flag} must be a positive integer."))
}

fn resolve_workspace_arg(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| format!("Could not resolve current directory: {err}"))?
            .join(path)
    };
    let metadata = std::fs::metadata(&absolute).map_err(|err| {
        format!(
            "Workspace path does not exist or cannot be read: {} ({err})",
            path.display()
        )
    })?;
    let workspace = if metadata.is_dir() {
        absolute
    } else if metadata.is_file() {
        absolute
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("File has no parent directory: {}", path.display()))?
    } else {
        return Err(format!(
            "Workspace path must be a file or directory: {}",
            path.display()
        ));
    };

    Ok(workspace.canonicalize().unwrap_or(workspace))
}

fn application_args_without_workspace() -> Vec<String> {
    vec![
        std::env::args()
            .next()
            .unwrap_or_else(|| "craic".to_string()),
    ]
}
