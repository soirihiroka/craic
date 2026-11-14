use crate::system::WorkspaceRef;
use crate::system::capabilities::shell::{ShellAccess, ShellCommandSpec, ShellRunRequest};
use semver::Version;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderUpdate {
    pub program: &'static str,
    pub label: &'static str,
    pub installed_version: Version,
    pub latest_version: Version,
    update_argument: &'static str,
}

#[derive(Debug)]
pub struct ProviderUpdateCheck {
    pub updates: Vec<ProviderUpdate>,
    pub checked_programs: Vec<&'static str>,
}

#[derive(Clone, Copy)]
struct Provider {
    program: &'static str,
    label: &'static str,
    latest_url: &'static str,
    update_argument: &'static str,
}

const PROVIDERS: [Provider; 4] = [
    Provider {
        program: "codex",
        label: "Codex CLI",
        latest_url: "https://registry.npmjs.org/@openai/codex/latest",
        update_argument: "update",
    },
    Provider {
        program: "grok",
        label: "Grok CLI",
        latest_url: "",
        update_argument: "update",
    },
    Provider {
        program: "opencode",
        label: "OpenCode",
        latest_url: "https://registry.npmjs.org/opencode-ai/latest",
        update_argument: "upgrade",
    },
    Provider {
        program: "agy",
        label: "AGY",
        latest_url: "https://api.github.com/repos/google-antigravity/antigravity-cli/releases/latest",
        update_argument: "update",
    },
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrokUpdate {
    current_version: String,
    latest_version: String,
    update_available: bool,
    error: Option<String>,
}

/// Check only installed CLIs. This runs on a worker, using the workspace's shell
/// so remote workspaces check their own installations and release channels.
pub fn check_updates(
    shell: Arc<dyn ShellAccess>,
    workspace: WorkspaceRef,
) -> Result<ProviderUpdateCheck, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent("Craic provider update checker")
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| error.to_string())?;
        let results = tokio::join!(
            check_provider(&client, shell.as_ref(), &workspace, PROVIDERS[0]),
            check_provider(&client, shell.as_ref(), &workspace, PROVIDERS[1]),
            check_provider(&client, shell.as_ref(), &workspace, PROVIDERS[2]),
            check_provider(&client, shell.as_ref(), &workspace, PROVIDERS[3]),
        );
        let mut updates = Vec::new();
        let mut checked_programs = Vec::new();
        for (provider, result) in PROVIDERS
            .into_iter()
            .zip([results.0, results.1, results.2, results.3])
        {
            match result {
                Ok(update) => {
                    checked_programs.push(provider.program);
                    updates.extend(update);
                }
                Err(error) => log::warn!(
                    "provider update check failed provider={}: {error}",
                    provider.program
                ),
            }
        }
        Ok(ProviderUpdateCheck {
            updates,
            checked_programs,
        })
    })
}

async fn check_provider(
    client: &reqwest::Client,
    shell: &dyn ShellAccess,
    workspace: &WorkspaceRef,
    provider: Provider,
) -> Result<Option<ProviderUpdate>, String> {
    // These are fixed program names; no release metadata is executed as shell code.
    let arguments = if provider.program == "grok" {
        "update --check --json"
    } else {
        "--version"
    };
    let script = format!(
        "command -v {0} >/dev/null 2>&1 || exit 127\nAGY_CLI_DISABLE_AUTO_UPDATE=true {0} {arguments}",
        provider.program,
    );
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        shell.run_script(ShellRunRequest::new(
            "check-provider-update",
            workspace.root.clone(),
            script,
        )),
    )
    .await
    .map_err(|_| "Version check timed out.".to_string())?
    .map_err(|error| error.to_string())??;
    if output.status_code == Some(127) {
        return Ok(None);
    }
    if !output.status_success(&[0]) {
        return Err(output.failure_message());
    }
    let stdout = output.stdout_text_trimmed();
    let (installed_version, latest_version) = if provider.program == "grok" {
        let status = stdout
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<GrokUpdate>(line).ok())
            .ok_or_else(|| "Grok returned no update status.".to_string())?;
        if let Some(error) = status.error {
            return Err(error);
        }
        if !status.update_available {
            return Ok(None);
        }
        (
            parse_version(&status.current_version)?,
            parse_version(&status.latest_version)?,
        )
    } else {
        let installed = stdout
            .split_whitespace()
            .find_map(|word| Version::parse(word.trim_start_matches('v')).ok())
            .ok_or_else(|| "Provider returned no version.".to_string())?;
        let response = client
            .get(provider.latest_url)
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .text()
            .await
            .map_err(|error| error.to_string())?;
        let metadata: serde_json::Value =
            serde_json::from_str(&response).map_err(|error| error.to_string())?;
        let latest = metadata
            .get("version")
            .or_else(|| metadata.get("tag_name"))
            .and_then(|version| version.as_str())
            .ok_or_else(|| "Release metadata contains no version.".to_string())?;
        (installed, parse_version(latest)?)
    };
    Ok(
        (latest_version > installed_version).then_some(ProviderUpdate {
            program: provider.program,
            label: provider.label,
            installed_version,
            latest_version,
            update_argument: provider.update_argument,
        }),
    )
}

fn parse_version(version: &str) -> Result<Version, String> {
    Version::parse(version.trim().trim_start_matches('v')).map_err(|error| error.to_string())
}

pub fn update_command(
    shell: &dyn ShellAccess,
    workspace: &WorkspaceRef,
    updates: &[ProviderUpdate],
) -> Result<ShellCommandSpec, String> {
    if updates.is_empty() {
        return Err("No provider updates are available.".to_string());
    }
    let mut script = String::from("craic_update_status=0\n");
    for update in updates {
        // Only fixed provider names and subcommands enter this script. Native
        // updaters retain their package-manager detection and interactive prompts.
        script.push_str(&format!(
            "printf '\\n%s\\n' 'Updating {}…'\nif {} {}; then printf '%s\\n' '{} updated.'; else craic_update_status=1; printf '%s\\n' '{} update failed.'; fi\n",
            update.label, update.program, update.update_argument, update.label, update.label,
        ));
    }
    script.push_str("exit \"$craic_update_status\"\n");
    shell.command(&workspace.root, "sh", &["-c".to_string(), script])
}
