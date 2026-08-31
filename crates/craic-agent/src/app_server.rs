use std::path::PathBuf;

use craic_codex_app_server::AppServerConfig;
use craic_system::system::capabilities::shell::ShellAccess;
use craic_system::system::{ProviderKind, WorkspaceRef};

pub fn config(
    shell: &dyn ShellAccess,
    workspace: &WorkspaceRef,
    provider_kind: ProviderKind,
) -> Result<AppServerConfig, String> {
    let codex = shell
        .which("codex")?
        .ok_or_else(|| "Codex is not installed on this workspace target".to_owned())?;
    let app_args = vec![
        "app-server".to_owned(),
        "--listen".to_owned(),
        "stdio://".to_owned(),
    ];
    let app = shell.fast_command(&workspace.root, &codex, &app_args)?;
    let version = shell.fast_command(&workspace.root, &codex, &["--version".to_owned()])?;
    let mut config = AppServerConfig {
        program: app.program,
        args: app.args,
        cwd: (provider_kind == ProviderKind::Local)
            .then(|| PathBuf::from(app.working_dir.absolute)),
        version_command: Some((version.program, version.args)),
        ..AppServerConfig::default()
    };
    config.capabilities.experimental_api = true;
    config.capabilities.mcp_server_openai_form_elicitation = true;
    Ok(config)
}
