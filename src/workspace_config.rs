use crate::github;
use crate::system::capabilities::files::{
    FileAccess, FileOperationEvent, FileRead, FileReadRequest, FileWriteMode, FileWritePayload,
    FileWriteRequest,
};
use crate::system::path::FileNodePath;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const REPO_CONFIG_DIR: &str = ".craic";
const REPO_CONFIG_FILE: &str = "config.toml";
const LOCAL_CONFIG_DIR: &str = ".craic/local";
const LOCAL_CONFIG_FILE: &str = "config.toml";
const LOCAL_GITIGNORE_FILE: &str = ".gitignore";
const LOCAL_GITIGNORE_CONTENTS: &str = "*\n";
const MAX_REPO_CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Default)]
pub(crate) struct GitConfig {
    pub(crate) commit_timezone: Option<String>,
    pub(crate) use_system_timezone: Option<bool>,
    pub(crate) warn_if_remote_owner_mismatch: Option<bool>,
    pub(crate) github_auth_account: Option<github::GitHubAuthAccount>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct QuickActionConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_target_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalWorkspaceConfig {
    #[serde(default)]
    git: LocalGitConfig,
    #[serde(default)]
    github: LocalGitHubConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quick_action: Option<LocalQuickActionConfig>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalGitConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    use_system_timezone: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warn_if_remote_owner_mismatch: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalGitHubConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_login: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalQuickActionConfig {
    #[serde(default)]
    actions: Vec<QuickActionConfig>,
}

pub(crate) fn git_config(path: &Path) -> GitConfig {
    let config = load(path);
    GitConfig {
        commit_timezone: config.git.commit_timezone,
        use_system_timezone: config.git.use_system_timezone,
        warn_if_remote_owner_mismatch: config.git.warn_if_remote_owner_mismatch,
        github_auth_account: local_github_auth_account(&config.github),
    }
}

pub(crate) fn git_config_from_file_access(files: &dyn FileAccess) -> GitConfig {
    let config = load_from_file_access(files);
    GitConfig {
        commit_timezone: config.git.commit_timezone,
        use_system_timezone: config.git.use_system_timezone,
        warn_if_remote_owner_mismatch: config.git.warn_if_remote_owner_mismatch,
        github_auth_account: local_github_auth_account(&config.github),
    }
}

pub(crate) fn save_git_config_with_file_access(
    files: &dyn FileAccess,
    commit_timezone: &str,
    warn_if_remote_owner_mismatch: bool,
    use_system_timezone: bool,
    github_auth_account: Option<&github::GitHubAuthAccount>,
) -> Result<(), String> {
    let mut config = load_from_file_access(files);
    apply_git_config(
        &mut config,
        commit_timezone,
        warn_if_remote_owner_mismatch,
        use_system_timezone,
        github_auth_account,
    )?;
    save_with_file_access(files, &config)
}

pub(crate) fn quick_action_config(path: &Path) -> Option<Vec<QuickActionConfig>> {
    load(path).quick_action.map(|config| config.actions)
}

pub(crate) fn save_quick_action_config(
    path: &Path,
    actions: Vec<QuickActionConfig>,
) -> Result<(), String> {
    let mut config = load(path);
    config.quick_action = Some(LocalQuickActionConfig { actions });
    save(path, &config)
}

pub(crate) fn markdown_lint_ignored_rules_from_file_access(files: &dyn FileAccess) -> Vec<String> {
    let config = load_repo_config_from_file_access(files)
        .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    config
        .get("markdown_lint")
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("ignored_rules"))
        .and_then(toml::Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::trim)
                .filter(|rule| !rule.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn add_markdown_lint_ignore_with_file_access(
    files: &dyn FileAccess,
    rule_name: &str,
) -> Result<String, String> {
    let rule_name = rule_name.trim();
    if rule_name.is_empty() {
        return Err("Markdown lint rule name is empty.".to_string());
    }

    let mut config = load_repo_config_from_file_access(files)?;
    let root = config
        .as_table_mut()
        .ok_or_else(|| "Repo config root must be a TOML table.".to_string())?;
    let markdown_lint = root
        .entry("markdown_lint")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let markdown_lint = markdown_lint
        .as_table_mut()
        .ok_or_else(|| "Repo config [markdown_lint] must be a TOML table.".to_string())?;
    let ignored_rules = markdown_lint
        .entry("ignored_rules")
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    let ignored_rules = ignored_rules
        .as_array_mut()
        .ok_or_else(|| "Repo config markdown_lint.ignored_rules must be an array.".to_string())?;

    if ignored_rules
        .iter()
        .filter_map(toml::Value::as_str)
        .any(|existing| existing.eq_ignore_ascii_case(rule_name))
    {
        return Ok(format!("{rule_name} is already ignored in repo config."));
    }
    ignored_rules.push(toml::Value::String(rule_name.to_string()));
    save_repo_config_with_file_access(files, &config)?;
    Ok(format!("Added {rule_name} to repo markdown lint ignores."))
}

pub(crate) fn commit_convention_from_file_access(files: &dyn FileAccess) -> Option<String> {
    let config_path = repo_config_node(files);
    let value = match load_repo_config_from_file_access(files) {
        Ok(value) => value,
        Err(err) => {
            log::warn!(
                "failed to parse commit convention from {}: {}",
                config_path.display(),
                err
            );
            return None;
        }
    };
    let Some(raw) = value.get("commit_convention") else {
        log::debug!("no commit_convention key in {}", config_path.display());
        return None;
    };
    let convention = match raw {
        toml::Value::String(value) => value.trim().to_string(),
        toml::Value::Array(values) => values
            .iter()
            .filter_map(toml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        _ => {
            log::warn!(
                "unsupported commit_convention type in {} (expected string or array)",
                config_path.display()
            );
            return None;
        }
    };
    let convention = convention.trim().to_string();
    if convention.is_empty() {
        log::warn!("commit_convention in {} is empty", config_path.display());
        return None;
    }
    log::info!("using commit_convention from {}", config_path.display());
    Some(convention)
}

pub(crate) fn normalize_timezone(value: &str) -> Result<String, String> {
    let value = value.trim();
    let compact = if value.len() == 6 && value.as_bytes()[3] == b':' {
        format!("{}{}", &value[..3], &value[4..])
    } else {
        value.to_string()
    };

    let bytes = compact.as_bytes();
    if compact.len() != 5 || !matches!(bytes[0], b'+' | b'-') {
        return Err("Commit timezone must look like +0000, -0500, or +09:30.".to_string());
    }
    if !bytes[1..].iter().all(u8::is_ascii_digit) {
        return Err("Commit timezone must look like +0000, -0500, or +09:30.".to_string());
    }

    let hours: u8 = compact[1..3]
        .parse()
        .map_err(|_| "Commit timezone hours must be between 00 and 23.".to_string())?;
    let minutes: u8 = compact[3..5]
        .parse()
        .map_err(|_| "Commit timezone minutes must be between 00 and 59.".to_string())?;

    if hours > 23 || minutes > 59 {
        return Err("Commit timezone must use hours 00-23 and minutes 00-59.".to_string());
    }

    Ok(compact)
}

fn apply_git_config(
    config: &mut LocalWorkspaceConfig,
    commit_timezone: &str,
    warn_if_remote_owner_mismatch: bool,
    use_system_timezone: bool,
    github_auth_account: Option<&github::GitHubAuthAccount>,
) -> Result<(), String> {
    let timezone = commit_timezone.trim();
    config.git.commit_timezone = if timezone.is_empty() {
        None
    } else {
        Some(normalize_timezone(timezone)?)
    };
    config.git.use_system_timezone = Some(use_system_timezone);
    config.git.warn_if_remote_owner_mismatch = Some(warn_if_remote_owner_mismatch);
    config.github = local_github_config(github_auth_account);
    Ok(())
}

fn load(path: &Path) -> LocalWorkspaceConfig {
    let config_path = config_path(path);

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return LocalWorkspaceConfig::default();
        }
        Err(err) => {
            log::warn!(
                "failed to read local workspace config path={} err={}",
                config_path.display(),
                err
            );
            return LocalWorkspaceConfig::default();
        }
    };

    parse_config(&contents, || config_path.display().to_string())
}

fn load_from_file_access(files: &dyn FileAccess) -> LocalWorkspaceConfig {
    let config_path = config_node(files);
    let contents = match read_text_via_callback(files, &config_path, None) {
        Ok(contents) => contents,
        Err(err) => {
            log::debug!(
                "failed to read local workspace config through file access workspace={} path={} err={}",
                files.workspace().display_name,
                config_path.display(),
                err
            );
            return LocalWorkspaceConfig::default();
        }
    };

    parse_config(&contents, || {
        format!(
            "{}:{}",
            files.workspace().display_name,
            config_path.display()
        )
    })
}

fn load_repo_config_from_file_access(files: &dyn FileAccess) -> Result<toml::Value, String> {
    let config_path = repo_config_node(files);
    let contents = match read_text_via_callback(files, &config_path, Some(MAX_REPO_CONFIG_BYTES)) {
        Ok(contents) => contents,
        Err(_) => return Ok(toml::Value::Table(toml::map::Map::new())),
    };
    toml::from_str(&contents).map_err(|err| {
        format!(
            "Failed to parse repo config {}: {err}",
            config_path.display()
        )
    })
}

fn read_text_via_callback(
    files: &dyn FileAccess,
    path: &FileNodePath,
    max_bytes: Option<u64>,
) -> Result<String, String> {
    read_with_info_via_callback(files, path, max_bytes)?.into_text()
}

fn read_with_info_via_callback(
    files: &dyn FileAccess,
    path: &FileNodePath,
    max_bytes: Option<u64>,
) -> Result<FileRead, String> {
    let (sender, receiver) = mpsc::channel();
    files.read_with_info(
        FileReadRequest {
            path: path.clone(),
            max_bytes,
            cancel_requested: None,
        },
        Box::new(move |event| {
            if let FileOperationEvent::Finished(result) = event {
                let _ = sender.send(result);
            }
        }),
    );
    receiver
        .recv()
        .map_err(|_| "Read operation did not return a result.".to_string())?
        .map_err(|err| err.to_string())
}

fn write_text_via_callback(
    files: &dyn FileAccess,
    path: &FileNodePath,
    contents: &str,
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    files.write_node(
        FileWriteRequest {
            path: path.clone(),
            mode: FileWriteMode::Replace,
            payload: FileWritePayload::File(contents.as_bytes().to_vec()),
            cancel_requested: None,
        },
        Box::new(move |event| {
            if let FileOperationEvent::Finished(result) = event {
                let _ = sender.send(result);
            }
        }),
    );
    receiver
        .recv()
        .map_err(|_| "Write operation did not return a result.".to_string())?
        .map_err(|err| err.to_string())
}

fn parse_config(contents: &str, label: impl FnOnce() -> String) -> LocalWorkspaceConfig {
    match toml::from_str::<LocalWorkspaceConfig>(contents) {
        Ok(config) => config,
        Err(err) => {
            log::warn!(
                "failed to parse local workspace config path={} err={}",
                label(),
                err
            );
            LocalWorkspaceConfig::default()
        }
    }
}

fn save(path: &Path, config: &LocalWorkspaceConfig) -> Result<(), String> {
    let config_path = config_path(path);
    let local_dir = config_path
        .parent()
        .ok_or_else(|| "Failed to resolve local workspace config directory.".to_string())?;

    std::fs::create_dir_all(local_dir).map_err(|err| {
        format!(
            "Failed to create local workspace config directory {}: {err}",
            local_dir.display()
        )
    })?;
    ensure_gitignore(local_dir)?;

    let contents = toml::to_string_pretty(config)
        .map_err(|err| format!("Failed to serialize local workspace config: {err}"))?;
    std::fs::write(&config_path, contents).map_err(|err| {
        format!(
            "Failed to write local workspace config {}: {err}",
            config_path.display()
        )
    })?;
    log::info!(
        "saved local workspace config path={}",
        config_path.display()
    );
    Ok(())
}

fn save_with_file_access(
    files: &dyn FileAccess,
    config: &LocalWorkspaceConfig,
) -> Result<(), String> {
    let local_dir = ensure_config_dir(files)?;
    let gitignore_path = ensure_file(files, &local_dir, LOCAL_GITIGNORE_FILE)?;
    write_text_via_callback(files, &gitignore_path, LOCAL_GITIGNORE_CONTENTS)?;
    log::debug!(
        "initialized local workspace gitignore through file access workspace={} path={}",
        files.workspace().display_name,
        gitignore_path.display()
    );

    let contents = toml::to_string_pretty(config)
        .map_err(|err| format!("Failed to serialize local workspace config: {err}"))?;
    let config_path = ensure_file(files, &local_dir, LOCAL_CONFIG_FILE)?;
    write_text_via_callback(files, &config_path, &contents)?;
    log::info!(
        "saved local workspace config through file access workspace={} path={}",
        files.workspace().display_name,
        config_path.display()
    );
    Ok(())
}

fn save_repo_config_with_file_access(
    files: &dyn FileAccess,
    config: &toml::Value,
) -> Result<(), String> {
    let craic_dir = ensure_dir(files, &files.root(), REPO_CONFIG_DIR)?;
    let config_path = ensure_file(files, &craic_dir, REPO_CONFIG_FILE)?;
    let contents = toml::to_string_pretty(config)
        .map_err(|err| format!("Failed to serialize repo config: {err}"))?;
    write_text_via_callback(files, &config_path, &contents)?;
    log::info!(
        "saved repo config through file access workspace={} path={}",
        files.workspace().display_name,
        config_path.display()
    );
    Ok(())
}

fn ensure_gitignore(local_dir: &Path) -> Result<(), String> {
    let gitignore_path = local_dir.join(LOCAL_GITIGNORE_FILE);
    std::fs::write(&gitignore_path, LOCAL_GITIGNORE_CONTENTS).map_err(|err| {
        format!(
            "Failed to write local workspace gitignore {}: {err}",
            gitignore_path.display()
        )
    })?;
    log::debug!(
        "initialized local workspace gitignore path={}",
        gitignore_path.display()
    );
    Ok(())
}

fn ensure_config_dir(files: &dyn FileAccess) -> Result<FileNodePath, String> {
    let root = files.root();
    let craic_dir = ensure_dir(files, &root, ".craic")?;
    ensure_dir(files, &craic_dir, "local")
}

fn ensure_dir(
    files: &dyn FileAccess,
    parent: &FileNodePath,
    name: &str,
) -> Result<FileNodePath, String> {
    let path = parent.join_child(name);
    match files.info(&path) {
        Ok(info) if info.kind.is_directory() => Ok(path),
        Ok(_) => Err(format!("{} is not a directory.", path.display())),
        Err(_) => {
            write_directory(files, &path)?;
            Ok(path)
        }
    }
}

fn ensure_file(
    files: &dyn FileAccess,
    parent: &FileNodePath,
    name: &str,
) -> Result<FileNodePath, String> {
    let path = parent.join_child(name);
    match files.info(&path) {
        Ok(info) if info.kind.is_file() => Ok(path),
        Ok(_) => Err(format!("{} is not a file.", path.display())),
        Err(_) => {
            write_empty_file(files, &path)?;
            Ok(path)
        }
    }
}

fn write_directory(files: &dyn FileAccess, path: &FileNodePath) -> Result<(), String> {
    write_node_via_callback(
        files,
        FileWriteRequest {
            path: path.clone(),
            mode: FileWriteMode::CreateNew,
            payload: FileWritePayload::Directory,
            cancel_requested: None,
        },
    )
}

fn write_empty_file(files: &dyn FileAccess, path: &FileNodePath) -> Result<(), String> {
    write_node_via_callback(
        files,
        FileWriteRequest {
            path: path.clone(),
            mode: FileWriteMode::CreateNew,
            payload: FileWritePayload::File(Vec::new()),
            cancel_requested: None,
        },
    )
}

fn write_node_via_callback(
    files: &dyn FileAccess,
    request: FileWriteRequest,
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    files.write_node(
        request,
        Box::new(move |event| {
            if let FileOperationEvent::Finished(result) = event {
                let _ = sender.send(result);
            }
        }),
    );
    receiver
        .recv()
        .map_err(|_| "Write operation did not return a result.".to_string())?
        .map_err(|err| err.to_string())
}

fn config_node(files: &dyn FileAccess) -> FileNodePath {
    files
        .root()
        .join_child(LOCAL_CONFIG_DIR)
        .join_child(LOCAL_CONFIG_FILE)
}

fn repo_config_node(files: &dyn FileAccess) -> FileNodePath {
    files
        .root()
        .join_child(REPO_CONFIG_DIR)
        .join_child(REPO_CONFIG_FILE)
}

fn config_path(path: &Path) -> PathBuf {
    path.join(LOCAL_CONFIG_DIR).join(LOCAL_CONFIG_FILE)
}

fn local_github_auth_account(config: &LocalGitHubConfig) -> Option<github::GitHubAuthAccount> {
    let host = config.auth_host.as_deref()?.trim();
    let login = config.auth_login.as_deref()?.trim();
    if host.is_empty() || login.is_empty() {
        return None;
    }

    Some(github::GitHubAuthAccount {
        host: host.to_string(),
        login: login.to_string(),
    })
}

fn local_github_config(account: Option<&github::GitHubAuthAccount>) -> LocalGitHubConfig {
    let Some(account) = account else {
        return LocalGitHubConfig::default();
    };
    let host = account.host.trim();
    let login = account.login.trim();
    if host.is_empty() || login.is_empty() {
        return LocalGitHubConfig::default();
    }

    LocalGitHubConfig {
        auth_host: Some(host.to_string()),
        auth_login: Some(login.to_string()),
    }
}
