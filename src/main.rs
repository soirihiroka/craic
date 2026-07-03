use adw::prelude::*;
use gtk::{gio, glib};
use std::path::{Path, PathBuf};
use std::time::Instant;

mod agent_provider;
mod ai_commit;
mod bitbucket;
mod config;
mod crash_log;
mod git;
mod github;
mod gitignore;
mod gitlab;
mod language_support;
mod markdown_lint;
mod quick_action;
mod spellcheck;
mod system;
mod terminal;
mod ui;
mod workspace;
mod workspace_config;

const APP_ID: &str = "dev.craic.Craic";

fn main() -> glib::ExitCode {
    let launch_start = Instant::now();
    let crash_log_dir = crash_log::install();
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
        ui::build_ui(
            app,
            launch_start,
            startup_workspace.workspace.clone(),
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
    error: Option<String>,
}

fn startup_workspace_arg() -> StartupWorkspaceArg {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        return StartupWorkspaceArg::default();
    }
    if args.len() > 1 {
        return StartupWorkspaceArg {
            workspace: None,
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
                error: None,
            }
        }
        Err(error) => StartupWorkspaceArg {
            workspace: None,
            error: Some(error),
        },
    }
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
