use std::path::Path;
use std::process::Command;

use super::GitHubAuthAccount;

pub fn switch_auth_account(path: &Path, account: Option<&GitHubAuthAccount>) -> Result<(), String> {
    let Some(account) = account else {
        return Ok(());
    };

    log::info!(
        "switching github auth account for workspace host={} account={}",
        account.host,
        account.login
    );
    let output = Command::new("gh")
        .arg("auth")
        .arg("switch")
        .arg("--hostname")
        .arg(&account.host)
        .arg("--user")
        .arg(&account.login)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run gh auth switch: {err}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() {
            format!("gh auth switch failed: {stdout}")
        } else {
            format!("gh auth switch failed: {stderr}")
        })
    }
}

pub fn checkout_pull_request(path: &Path, number: u32) -> Result<String, String> {
    let output = Command::new("gh")
        .current_dir(path)
        .arg("pr")
        .arg("checkout")
        .arg(number.to_string())
        .output()
        .map_err(|err| format!("Failed to run gh pr checkout {number}: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() {
            format!("gh pr checkout {number} failed: {stdout}")
        } else {
            format!("gh pr checkout {number} failed: {stderr}")
        })
    }
}

pub(crate) fn ssh_switch_auth_account_script(quoted_host: &str, quoted_login: &str) -> String {
    format!(
        "{} && \"$gh_cmd\" auth switch --hostname {quoted_host} --user {quoted_login}",
        ssh_gh_command_assignment()
    )
}

pub(crate) fn ssh_checkout_pull_request_script(quoted_root: &str, quoted_number: &str) -> String {
    format!(
        "cd {quoted_root} && {} && \"$gh_cmd\" pr checkout {quoted_number}",
        ssh_gh_command_assignment()
    )
}

fn ssh_gh_command_assignment() -> &'static str {
    "gh_cmd=$(command -v gh 2>/dev/null || { if [ -x /home/linuxbrew/.linuxbrew/bin/gh ]; then printf '%s\\n' /home/linuxbrew/.linuxbrew/bin/gh; elif [ -x \"$HOME/.local/bin/gh\" ]; then printf '%s\\n' \"$HOME/.local/bin/gh\"; else printf '%s\\n' gh; fi; })"
}
