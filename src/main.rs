use adw::prelude::*;
use gtk::{gio, glib};
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
    app.connect_activate(move |app| ui::build_ui(app, launch_start));
    let exit_code = app.run();
    log::info!(
        "application exit total_ms={}",
        launch_start.elapsed().as_millis()
    );
    exit_code
}
