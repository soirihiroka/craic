use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moka::sync::Cache;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

const GITHUB_AVATAR_URL_CACHE_TTL_SECS: i64 = 60 * 60 * 24 * 30;
const GITHUB_AVATAR_BYTES_CACHE_TTL_SECS: i64 = 60 * 60 * 24 * 30;
const MAX_AVATAR_CACHE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHubRepoMetadata {
    Fork,
    Private,
    Public,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestInfo {
    pub number: u32,
    pub title: String,
    pub author: String,
    pub created_at: String,
    pub is_draft: bool,
    pub head_ref_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubAuthAccount {
    pub host: String,
    pub login: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubRepositoryOwner {
    pub host: String,
    pub auth_login: String,
    pub owner: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubPublishRepositoryRequest {
    pub host: String,
    pub auth_login: String,
    pub owner: String,
    pub name: String,
    pub private: bool,
}

#[derive(Deserialize)]
struct GhPullRequestRow {
    number: u32,
    title: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    author: Option<GhPullRequestAuthor>,
}

#[derive(Deserialize)]
struct GhPullRequestAuthor {
    login: Option<String>,
}

#[derive(Deserialize)]
struct GhOrganization {
    login: Option<String>,
}

impl GitHubRepoMetadata {
    fn cache_value(self) -> &'static str {
        match self {
            Self::Fork => "fork",
            Self::Private => "private",
            Self::Public => "public",
        }
    }
}

pub fn avatar_url_for_login(login: &str) -> String {
    let login = percent_encode_path_segment(login.trim());
    format!("https://github.com/{login}.png?size=64")
}

pub fn avatar_url_for_email(email: &str) -> Result<String, String> {
    let email = email.trim();
    if email.is_empty() {
        return Err("Email is required to resolve a GitHub avatar.".to_string());
    }

    let cache_key = email.to_ascii_lowercase();
    if let Some(url) = avatar_url_cache().get(&cache_key) {
        return Ok(url);
    }

    if let Some(url) = cached_avatar_url(&cache_key) {
        avatar_url_cache().insert(cache_key, url.clone());
        return Ok(url);
    }

    let url = login_for_email(email).map(|login| avatar_url_for_login(&login))?;
    avatar_url_cache().insert(cache_key.clone(), url.clone());
    cache_avatar_url(&cache_key, &url);
    Ok(url)
}

pub fn login_for_email(email: &str) -> Result<String, String> {
    let email = email.trim();
    if let Some(login) = login_from_noreply_email(email) {
        return Ok(login);
    }

    let query = format!("{email} in:email");
    let output = Command::new("gh")
        .arg("api")
        .arg("-X")
        .arg("GET")
        .arg("search/users")
        .arg("-f")
        .arg(format!("q={query}"))
        .arg("--jq")
        .arg(".items[0].login")
        .output()
        .map_err(|err| format!("Failed to run gh: {err}"))?;

    if output.status.success() {
        let login = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if login.is_empty() || login == "null" {
            Err("No GitHub user found for email.".to_string())
        } else {
            Ok(login)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            "Failed to resolve GitHub user.".to_string()
        } else {
            stderr
        })
    }
}

pub fn login_from_noreply_email(email: &str) -> Option<String> {
    let email = email.trim();
    let local = email.strip_suffix("@users.noreply.github.com")?;
    local
        .split_once('+')
        .map(|(_, login)| login)
        .or(Some(local))
        .map(str::to_string)
        .filter(|login| !login.is_empty())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitEmailOption {
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

pub fn commit_email_options() -> Result<Vec<CommitEmailOption>, String> {
    log::debug!("loading commit email options from gh");

    let mut options = Vec::new();
    let mut errors = Vec::new();

    match gh_authenticated_accounts() {
        Ok(accounts) if !accounts.is_empty() => {
            log::debug!("loaded gh authenticated accounts count={}", accounts.len());
            for account in accounts {
                collect_account_commit_emails(&account, &mut options, &mut errors);
            }
        }
        Ok(_) => {
            log::warn!("gh auth status returned no accounts; falling back to active account");
            errors.push("gh auth status returned no accounts.".to_string());
            collect_active_account_commit_emails(&mut options, &mut errors);
        }
        Err(err) => {
            log::warn!("failed to load gh authenticated accounts: {err}");
            errors.push(err);
            collect_active_account_commit_emails(&mut options, &mut errors);
        }
    }

    if options.is_empty() {
        let details = errors
            .into_iter()
            .map(|err| err.trim().to_string())
            .filter(|err| !err.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        let message = if details.is_empty() {
            "gh did not return any email addresses.".to_string()
        } else {
            format!("gh did not return any email addresses. {details}")
        };
        log::warn!("{message}");
        Err(message)
    } else {
        log::debug!("loaded commit email options count={}", options.len());
        cache_commit_email_options(&options);
        Ok(options)
    }
}

pub fn cached_commit_email_options() -> Option<Vec<CommitEmailOption>> {
    match commit_email_options_cache().lock() {
        Ok(cached) if cached.is_empty() => None,
        Ok(cached) => {
            let options = cached.clone();
            log::debug!("using cached commit email options count={}", options.len());
            Some(options)
        }
        Err(err) => {
            log::warn!("failed to read cached commit email options: {err}");
            None
        }
    }
}

fn cache_commit_email_options(options: &[CommitEmailOption]) {
    if options.is_empty() {
        return;
    }

    match commit_email_options_cache().lock() {
        Ok(mut cached) => {
            cached.clear();
            cached.extend(options.iter().cloned());
            log::debug!("cached commit email options count={}", cached.len());
        }
        Err(err) => {
            log::warn!("failed to cache commit email options: {err}");
        }
    }
}

fn commit_email_options_cache() -> &'static Mutex<Vec<CommitEmailOption>> {
    static COMMIT_EMAIL_OPTIONS_CACHE: OnceLock<Mutex<Vec<CommitEmailOption>>> = OnceLock::new();

    COMMIT_EMAIL_OPTIONS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GitHubAccount {
    host: String,
    login: String,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: Option<String>,
    name: Option<String>,
    avatar_url: Option<String>,
    id: Option<u64>,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubEmail {
    email: Option<String>,
}

fn gh_authenticated_accounts() -> Result<Vec<GitHubAccount>, String> {
    let output = Command::new("gh")
        .arg("auth")
        .arg("status")
        .arg("--json")
        .arg("hosts")
        .output()
        .map_err(|err| format!("Failed to run gh auth status: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("gh auth status failed: {stdout}")
        } else {
            format!("gh auth status failed: {stderr}")
        });
    }

    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .map_err(|err| format!("Failed to parse gh auth status output: {err}"))?;
    let mut accounts = Vec::new();
    collect_authenticated_accounts(&value, None, &mut accounts);
    Ok(accounts)
}

fn collect_authenticated_accounts(
    value: &serde_json::Value,
    host: Option<&str>,
    accounts: &mut Vec<GitHubAccount>,
) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_authenticated_accounts(value, host, accounts);
            }
        }
        serde_json::Value::Object(object) => {
            let object_host = object
                .get("host")
                .or_else(|| object.get("Host"))
                .or_else(|| object.get("hostname"))
                .or_else(|| object.get("Hostname"))
                .and_then(serde_json::Value::as_str)
                .or(host);
            if let Some(login) = login_from_account_object(object) {
                push_account(accounts, object_host.unwrap_or("github.com"), login);
            }

            if let Some(host) = object_host {
                for key in ["users", "Users", "accounts", "Accounts"] {
                    if let Some(value) = object.get(key) {
                        collect_named_accounts(value, host, accounts);
                    }
                }
            }

            for (key, value) in object {
                let child_host = if is_probable_host_key(key) {
                    Some(key.as_str())
                } else {
                    object_host
                };
                collect_authenticated_accounts(value, child_host, accounts);
            }
        }
        _ => {}
    }
}

fn login_from_account_object(object: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    object
        .get("login")
        .or_else(|| object.get("Login"))
        .or_else(|| object.get("account"))
        .or_else(|| object.get("Account"))
        .or_else(|| object.get("user"))
        .or_else(|| object.get("User"))
        .or_else(|| object.get("username"))
        .or_else(|| object.get("Username"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|login| !login.is_empty())
}

fn collect_named_accounts(
    value: &serde_json::Value,
    host: &str,
    accounts: &mut Vec<GitHubAccount>,
) {
    match value {
        serde_json::Value::Object(object) => {
            if login_from_account_object(object).is_some() {
                collect_authenticated_accounts(value, Some(host), accounts);
                return;
            }

            for (login, value) in object {
                let login = login.trim();
                if !login.is_empty() && !is_probable_host_key(login) {
                    push_account(accounts, host, login);
                }
                collect_authenticated_accounts(value, Some(host), accounts);
            }
        }
        value => collect_authenticated_accounts(value, Some(host), accounts),
    }
}

fn is_probable_host_key(key: &str) -> bool {
    key.contains('.') || key.eq_ignore_ascii_case("github.com")
}

fn push_account(accounts: &mut Vec<GitHubAccount>, host: &str, login: &str) {
    let host = host.trim();
    let login = login.trim();
    if host.is_empty() || login.is_empty() {
        return;
    }

    if accounts
        .iter()
        .any(|account| account.host == host && account.login == login)
    {
        return;
    }

    accounts.push(GitHubAccount {
        host: host.to_string(),
        login: login.to_string(),
    });
}

fn collect_account_commit_emails(
    account: &GitHubAccount,
    options: &mut Vec<CommitEmailOption>,
    errors: &mut Vec<String>,
) {
    match gh_auth_token(account) {
        Ok(token) => collect_token_commit_emails(account, &token, options, errors),
        Err(err) => {
            log::warn!(
                "failed to get gh token for account={} host={}: {err}",
                account.login,
                account.host
            );
            errors.push(err);
            collect_public_noreply_emails(account, options, errors);
        }
    }
}

fn collect_active_account_commit_emails(
    options: &mut Vec<CommitEmailOption>,
    errors: &mut Vec<String>,
) {
    let mut display_name = None;
    let mut avatar_url = None;
    match gh_api_json::<GitHubUser>("user") {
        Ok(user) => {
            let login = user.login;
            let name = user.name;
            let user_avatar_url = user.avatar_url;
            display_name = display_name_for_user(name.as_deref(), login.as_deref());
            avatar_url = user_avatar_url;
            push_user_commit_emails(
                login.as_deref(),
                display_name.as_deref(),
                avatar_url.as_deref(),
                user.id,
                user.email,
                options,
            );
        }
        Err(err) => errors.push(err),
    }

    for endpoint in ["user/emails", "user/public_emails"] {
        match gh_api_json::<Vec<GitHubEmail>>(endpoint) {
            Ok(entries) => push_email_entries(
                options,
                entries,
                display_name.as_deref(),
                avatar_url.as_deref(),
            ),
            Err(err) => errors.push(err),
        }
    }
}

fn collect_token_commit_emails(
    account: &GitHubAccount,
    token: &str,
    options: &mut Vec<CommitEmailOption>,
    errors: &mut Vec<String>,
) {
    let mut display_name = Some(account.login.clone());
    let mut avatar_url = None;
    match gh_api_json_for_account::<GitHubUser>(account, token, "user") {
        Ok(user) => {
            let login = user.login;
            let name = user.name;
            let user_avatar_url = user.avatar_url;
            let login = login.as_deref().or(Some(account.login.as_str()));
            display_name = display_name_for_user(name.as_deref(), login);
            avatar_url = user_avatar_url;
            push_user_commit_emails(
                login,
                display_name.as_deref(),
                avatar_url.as_deref(),
                user.id,
                user.email,
                options,
            );
        }
        Err(err) => {
            log::warn!(
                "failed to load gh user for account={} host={}: {err}",
                account.login,
                account.host
            );
            errors.push(err);
            collect_public_noreply_emails(account, options, errors);
        }
    }

    for endpoint in ["user/emails", "user/public_emails"] {
        match gh_api_json_for_account::<Vec<GitHubEmail>>(account, token, endpoint) {
            Ok(entries) => push_email_entries(
                options,
                entries,
                display_name.as_deref(),
                avatar_url.as_deref(),
            ),
            Err(err) => {
                log::warn!(
                    "failed to load gh email endpoint={} account={} host={}: {err}",
                    endpoint,
                    account.login,
                    account.host
                );
                errors.push(err);
            }
        }
    }
}

fn collect_public_noreply_emails(
    account: &GitHubAccount,
    options: &mut Vec<CommitEmailOption>,
    errors: &mut Vec<String>,
) {
    let endpoint = format!("users/{}", account.login);
    match gh_api_json_for_host::<GitHubUser>(&account.host, &endpoint) {
        Ok(user) => {
            let avatar_url = user.avatar_url;
            let display_name =
                display_name_for_user(user.name.as_deref(), Some(account.login.as_str()));
            push_user_commit_emails(
                Some(account.login.as_str()),
                display_name.as_deref(),
                avatar_url.as_deref(),
                user.id,
                None,
                options,
            );
        }
        Err(err) => {
            log::warn!(
                "failed to load public gh user for account={} host={}: {err}",
                account.login,
                account.host
            );
            errors.push(err);
            push_user_commit_emails(
                Some(account.login.as_str()),
                Some(account.login.as_str()),
                None,
                None,
                None,
                options,
            );
        }
    }
}

fn push_user_commit_emails(
    login: Option<&str>,
    name: Option<&str>,
    avatar_url: Option<&str>,
    id: Option<u64>,
    email: Option<String>,
    options: &mut Vec<CommitEmailOption>,
) {
    if let Some(email) = email {
        push_email_option(options, email, name, avatar_url);
    }

    if let Some(login) = login.map(str::trim).filter(|login| !login.is_empty()) {
        if let Some(id) = id {
            push_email_option(
                options,
                format!("{id}+{login}@users.noreply.github.com"),
                name,
                avatar_url,
            );
        }
        push_email_option(
            options,
            format!("{login}@users.noreply.github.com"),
            name,
            avatar_url,
        );
    }
}

fn push_email_entries(
    options: &mut Vec<CommitEmailOption>,
    entries: Vec<GitHubEmail>,
    name: Option<&str>,
    avatar_url: Option<&str>,
) {
    for entry in entries {
        if let Some(email) = entry.email {
            push_email_option(options, email, name, avatar_url);
        }
    }
}

fn gh_auth_token(account: &GitHubAccount) -> Result<String, String> {
    let output = Command::new("gh")
        .arg("auth")
        .arg("token")
        .arg("--hostname")
        .arg(&account.host)
        .arg("--user")
        .arg(&account.login)
        .output()
        .map_err(|err| {
            format!(
                "Failed to run gh auth token for {} on {}: {err}",
                account.login, account.host
            )
        })?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            Err(format!(
                "gh auth token returned no token for {} on {}.",
                account.login, account.host
            ))
        } else {
            Ok(token)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!(
                "gh auth token failed for {} on {}.",
                account.login, account.host
            )
        } else {
            format!(
                "gh auth token failed for {} on {}: {stderr}",
                account.login, account.host
            )
        })
    }
}

fn gh_api_json<T: for<'de> Deserialize<'de>>(endpoint: &str) -> Result<T, String> {
    let output = Command::new("gh")
        .arg("api")
        .arg(endpoint)
        .output()
        .map_err(|err| format!("Failed to run gh: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("gh api {endpoint} failed: {stdout}")
        } else {
            format!("gh api {endpoint} failed: {stderr}")
        });
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("Failed to parse gh api {endpoint} output: {err}"))
}

fn gh_api_json_for_host<T: for<'de> Deserialize<'de>>(
    host: &str,
    endpoint: &str,
) -> Result<T, String> {
    let output = Command::new("gh")
        .arg("api")
        .arg("--hostname")
        .arg(host)
        .arg(endpoint)
        .output()
        .map_err(|err| format!("Failed to run gh api {endpoint} for {host}: {err}"))?;

    parse_gh_api_output(output, host, endpoint)
}

fn gh_api_json_for_account<T: for<'de> Deserialize<'de>>(
    account: &GitHubAccount,
    token: &str,
    endpoint: &str,
) -> Result<T, String> {
    let output = Command::new("gh")
        .arg("api")
        .arg("--hostname")
        .arg(&account.host)
        .arg(endpoint)
        .env("GH_TOKEN", token)
        .env("GH_ENTERPRISE_TOKEN", token)
        .output()
        .map_err(|err| {
            format!(
                "Failed to run gh api {endpoint} for {} on {}: {err}",
                account.login, account.host
            )
        })?;

    parse_gh_api_output(output, &account.host, endpoint)
}

fn apply_gh_auth_account(
    command: &mut Command,
    account: Option<&GitHubAuthAccount>,
) -> Result<(), String> {
    let Some(account) = account else {
        return Ok(());
    };
    let account = GitHubAccount {
        host: account.host.trim().to_string(),
        login: account.login.trim().to_string(),
    };
    if account.host.is_empty() || account.login.is_empty() {
        return Ok(());
    }

    let token = gh_auth_token(&account)?;
    command
        .env("GH_TOKEN", &token)
        .env("GH_ENTERPRISE_TOKEN", token)
        .env("GH_HOST", &account.host);
    log::debug!(
        "using workspace github auth account={} host={}",
        account.login,
        account.host
    );
    Ok(())
}

fn parse_gh_api_output<T: for<'de> Deserialize<'de>>(
    output: std::process::Output,
    host: &str,
    endpoint: &str,
) -> Result<T, String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("gh api {endpoint} failed on {host}: {stdout}")
        } else {
            format!("gh api {endpoint} failed on {host}: {stderr}")
        });
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("Failed to parse gh api {endpoint} output from {host}: {err}"))
}

fn display_name_for_user(name: Option<&str>, login: Option<&str>) -> Option<String> {
    name.and_then(|name| {
        let name = name.trim();
        if name.is_empty() { None } else { Some(name) }
    })
    .or_else(|| {
        login.and_then(|login| {
            let login = login.trim();
            if login.is_empty() { None } else { Some(login) }
        })
    })
    .map(str::to_string)
}

fn push_email_option(
    options: &mut Vec<CommitEmailOption>,
    email: String,
    name: Option<&str>,
    avatar_url: Option<&str>,
) {
    let email = email.trim();
    if email.is_empty() || !email.contains('@') {
        return;
    }
    let name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("")
        .to_string();
    let avatar_url = avatar_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string);

    if let Some(index) = options
        .iter()
        .position(|existing| existing.email.eq_ignore_ascii_case(email))
    {
        if options[index].name.is_empty() && !name.is_empty() {
            options[index].name = name;
        }
        if options[index].avatar_url.is_none() && avatar_url.is_some() {
            options[index].avatar_url = avatar_url;
        }
        return;
    }

    options.push(CommitEmailOption {
        email: email.to_string(),
        name,
        avatar_url,
    });
}

pub fn download_avatar(url: &str) -> Result<Vec<u8>, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| format!("Failed to create HTTP client: {err}"))?
        .get(url)
        .header("User-Agent", "Craic")
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| format!("Failed to fetch avatar: {err}"))?
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|err| format!("Failed to read avatar: {err}"))
}

pub fn cached_avatar_bytes(cache_key: &str) -> Option<Vec<u8>> {
    let Some((bytes, updated_at)) = cached_avatar_bytes_row(cache_key) else {
        return None;
    };

    if cache_expired(updated_at, GITHUB_AVATAR_BYTES_CACHE_TTL_SECS) {
        log::debug!(
            "github avatar disk cache expired key={} age_secs={}",
            cache_key,
            cache_age_secs(updated_at)
        );
        return None;
    }

    log::debug!(
        "github avatar disk cache hit key={} bytes={} age_secs={}",
        cache_key,
        bytes.len(),
        cache_age_secs(updated_at)
    );
    Some(bytes)
}

pub fn cache_avatar_bytes(cache_key: &str, bytes: &[u8]) {
    if cache_key.trim().is_empty() || bytes.is_empty() || bytes.len() > MAX_AVATAR_CACHE_BYTES {
        return;
    }

    let Ok(conn) = network_cache_connection() else {
        log::warn!("failed to open network cache for github avatar write");
        return;
    };

    match conn.execute(
        "INSERT INTO github_avatar_cache (cache_key, bytes, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(cache_key) DO UPDATE SET
            bytes = excluded.bytes,
            updated_at = excluded.updated_at",
        params![cache_key, bytes, unix_now_secs()],
    ) {
        Ok(_) => log::debug!(
            "github avatar disk cache write key={} bytes={}",
            cache_key,
            bytes.len()
        ),
        Err(err) => log::warn!("failed to write github avatar disk cache key={cache_key}: {err}"),
    }
}

pub fn parse_github_url(url: &str) -> Option<String> {
    if url.contains("github.com") {
        let cleaned = if let Some(idx) = url.find("github.com:") {
            &url[idx + 11..]
        } else if let Some(idx) = url.find("github.com/") {
            &url[idx + 11..]
        } else {
            url
        };
        let cleaned = cleaned.strip_suffix(".git").unwrap_or(cleaned);
        let parts: Vec<&str> = cleaned.split('/').collect();
        if parts.len() >= 2 {
            return Some(format!("{}/{}", parts[0], parts[1]));
        }
    }
    None
}

pub fn fetch_repo_metadata(repo_slug: &str) -> Result<GitHubRepoMetadata, String> {
    fetch_repo_metadata_with_account(repo_slug, None)
}

pub fn fetch_repo_metadata_with_account(
    repo_slug: &str,
    account: Option<&GitHubAuthAccount>,
) -> Result<GitHubRepoMetadata, String> {
    let repo_slug = repo_slug.trim();
    if repo_slug.is_empty() {
        return Err("GitHub repository slug is required.".to_string());
    }

    let mut command = Command::new("gh");
    command
        .arg("repo")
        .arg("view")
        .arg(repo_slug)
        .arg("--json")
        .arg("isFork,isPrivate")
        .arg("--jq")
        .arg(repo_metadata_jq());
    apply_gh_auth_account(&mut command, account)?;

    let output = command
        .output()
        .map_err(|err| format!("Failed to run gh repo view for {repo_slug}: {err}"))?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        parse_repo_metadata_value(&value).ok_or_else(|| {
            format!("Invalid gh repository metadata response for {repo_slug}: {value}")
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() {
            format!("gh repo view {repo_slug} failed: {stdout}")
        } else {
            format!("gh repo view {repo_slug} failed: {stderr}")
        })
    }
}

pub fn open_pull_requests(workspace_root: &str) -> Result<Vec<PullRequestInfo>, String> {
    open_pull_requests_with_account(workspace_root, None)
}

pub fn open_pull_requests_with_account(
    workspace_root: &str,
    account: Option<&GitHubAuthAccount>,
) -> Result<Vec<PullRequestInfo>, String> {
    let mut command = Command::new("gh");
    command
        .current_dir(workspace_root)
        .arg("pr")
        .arg("list")
        .arg("--state")
        .arg("open")
        .arg("--json")
        .arg("number,title,author,createdAt,isDraft,headRefName");
    apply_gh_auth_account(&mut command, account)?;

    let output = command
        .output()
        .map_err(|err| format!("Failed to run gh pr list: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("gh pr list failed: {stdout}")
        } else {
            format!("gh pr list failed: {stderr}")
        });
    }

    parse_pull_requests(&output.stdout)
}

pub fn authenticated_accounts() -> Result<Vec<GitHubAuthAccount>, String> {
    gh_authenticated_accounts().map(|accounts| {
        accounts
            .into_iter()
            .map(|account| GitHubAuthAccount {
                host: account.host,
                login: account.login,
            })
            .collect()
    })
}

pub fn repository_owners_for_account(
    account: &GitHubAuthAccount,
) -> Result<Vec<GitHubRepositoryOwner>, String> {
    let account = GitHubAccount {
        host: account.host.clone(),
        login: account.login.clone(),
    };
    let mut owners = vec![GitHubRepositoryOwner {
        host: account.host.clone(),
        auth_login: account.login.clone(),
        owner: account.login.clone(),
    }];

    match gh_auth_token(&account).and_then(|token| {
        gh_api_json_for_account::<Vec<GhOrganization>>(&account, &token, "user/orgs")
    }) {
        Ok(orgs) => {
            for org in orgs {
                let Some(login) = org.login.map(|login| login.trim().to_string()) else {
                    continue;
                };
                if login.is_empty() || owners.iter().any(|owner| owner.owner == login) {
                    continue;
                }
                owners.push(GitHubRepositoryOwner {
                    host: account.host.clone(),
                    auth_login: account.login.clone(),
                    owner: login,
                });
            }
        }
        Err(err) => {
            log::warn!(
                "failed to load github publish owners account={} host={}: {err}",
                account.login,
                account.host
            );
        }
    }

    Ok(owners)
}

pub fn publish_repository(
    workspace_root: &str,
    request: &GitHubPublishRepositoryRequest,
) -> Result<String, String> {
    let host = request.host.trim();
    let auth_login = request.auth_login.trim();
    let owner = request.owner.trim();
    let name = request.name.trim();
    if host.is_empty() {
        return Err("GitHub host is required.".to_string());
    }
    if auth_login.is_empty() {
        return Err("GitHub account is required.".to_string());
    }
    if owner.is_empty() {
        return Err("Repository owner is required.".to_string());
    }
    if name.is_empty() {
        return Err("Repository name is required.".to_string());
    }

    if repository_exists(request)? {
        return Err(format!(
            "Repository {owner}/{name} already exists on {host}."
        ));
    }

    let account = GitHubAccount {
        host: host.to_string(),
        login: auth_login.to_string(),
    };
    let token = gh_auth_token(&account)?;
    let repo_slug = format!("{owner}/{name}");
    log::info!(
        "github publish repository start host={} account={} repo={} workspace={}",
        host,
        auth_login,
        repo_slug,
        workspace_root
    );

    let mut command = Command::new("gh");
    command
        .current_dir(workspace_root)
        .arg("repo")
        .arg("create")
        .arg(&repo_slug)
        .arg("--source")
        .arg(workspace_root)
        .arg("--remote")
        .arg("origin")
        .arg("--push")
        .arg(if request.private {
            "--private"
        } else {
            "--public"
        })
        .env("GH_TOKEN", &token)
        .env("GH_ENTERPRISE_TOKEN", &token)
        .env("GH_HOST", host);

    let output = command
        .output()
        .map_err(|err| format!("Failed to run gh repo create: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        log::info!(
            "github publish repository complete host={} account={} repo={}",
            host,
            auth_login,
            repo_slug
        );
        if !stdout.is_empty() {
            Ok(stdout)
        } else if !stderr.is_empty() {
            Ok(stderr)
        } else {
            Ok(format!("Published {repo_slug}."))
        }
    } else {
        let message = if stderr.is_empty() { stdout } else { stderr };
        log::warn!(
            "github publish repository failed host={} account={} repo={} error={}",
            host,
            auth_login,
            repo_slug,
            message
        );
        Err(if message.is_empty() {
            "gh repo create failed.".to_string()
        } else {
            message
        })
    }
}

pub fn repository_exists(request: &GitHubPublishRepositoryRequest) -> Result<bool, String> {
    let host = request.host.trim();
    let auth_login = request.auth_login.trim();
    let owner = request.owner.trim();
    let name = request.name.trim();
    if host.is_empty() || auth_login.is_empty() || owner.is_empty() || name.is_empty() {
        return Ok(false);
    }

    let account = GitHubAccount {
        host: host.to_string(),
        login: auth_login.to_string(),
    };
    let token = gh_auth_token(&account)?;
    let repo_slug = format!("{owner}/{name}");
    let output = Command::new("gh")
        .arg("repo")
        .arg("view")
        .arg(&repo_slug)
        .arg("--json")
        .arg("name")
        .env("GH_TOKEN", &token)
        .env("GH_ENTERPRISE_TOKEN", &token)
        .env("GH_HOST", host)
        .output()
        .map_err(|err| format!("Failed to run gh repo view for {repo_slug}: {err}"))?;

    if output.status.success() {
        return Ok(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = if stderr.is_empty() { stdout } else { stderr };
    let lower = message.to_lowercase();
    if lower.contains("could not resolve to a repository")
        || lower.contains("not found")
        || lower.contains("http 404")
    {
        Ok(false)
    } else {
        Err(if message.is_empty() {
            format!("gh repo view {repo_slug} failed.")
        } else {
            format!("gh repo view {repo_slug} failed: {message}")
        })
    }
}

pub fn parse_pull_requests(bytes: &[u8]) -> Result<Vec<PullRequestInfo>, String> {
    let rows: Vec<GhPullRequestRow> = serde_json::from_slice(bytes)
        .map_err(|err| format!("Failed to parse gh pr list output: {err}"))?;
    Ok(rows
        .into_iter()
        .map(|row| PullRequestInfo {
            number: row.number,
            title: row.title,
            author: row
                .author
                .and_then(|author| author.login)
                .unwrap_or_else(|| "unknown".to_string()),
            created_at: row.created_at,
            is_draft: row.is_draft,
            head_ref_name: row.head_ref_name,
        })
        .collect())
}

pub fn repo_metadata_for_workspace<F>(
    workspace_id: &str,
    workspace_root: &str,
    repo_slug: &str,
    remote_name: Option<&str>,
    remote_url: Option<&str>,
    fetch: F,
) -> Result<GitHubRepoMetadata, String>
where
    F: FnOnce() -> Result<GitHubRepoMetadata, String>,
{
    let workspace_id = workspace_id.trim();
    let workspace_root = workspace_root.trim();
    let repo_slug = repo_slug.trim();
    let cache_key = repo_metadata_cache_key(repo_slug);

    if let Some(cached) = cached_repo_metadata(&cache_key) {
        log::debug!(
            "github repo metadata disk cache hit workspace_id={} repo={} remote={} age_secs={}",
            workspace_id,
            repo_slug,
            remote_url.unwrap_or_default(),
            cache_age_secs(cached.updated_at)
        );
        return Ok(cached.metadata);
    }

    log::debug!(
        "github repo metadata disk cache miss workspace_id={} repo={} remote={}",
        workspace_id,
        repo_slug,
        remote_url.unwrap_or_default()
    );

    match fetch() {
        Ok(metadata) => {
            cache_repo_metadata(
                &cache_key,
                workspace_id,
                workspace_root,
                repo_slug,
                remote_name,
                remote_url,
                metadata,
            );
            Ok(metadata)
        }
        Err(err) => {
            if let Some(cached) = cached_repo_metadata(&cache_key) {
                log::warn!(
                    "github repo metadata fetch failed; using stale cache workspace_id={} repo={} age_secs={} err={}",
                    workspace_id,
                    repo_slug,
                    cache_age_secs(cached.updated_at),
                    err
                );
                return Ok(cached.metadata);
            }
            Err(err)
        }
    }
}

pub fn repo_metadata_jq() -> &'static str {
    "if .isFork then \"fork\" elif .isPrivate then \"private\" else \"public\" end"
}

pub fn parse_repo_metadata_value(value: &str) -> Option<GitHubRepoMetadata> {
    match value {
        "fork" => Some(GitHubRepoMetadata::Fork),
        "private" => Some(GitHubRepoMetadata::Private),
        "public" => Some(GitHubRepoMetadata::Public),
        _ => None,
    }
}

fn avatar_url_cache() -> &'static Cache<String, String> {
    static AVATAR_URL_CACHE: OnceLock<Cache<String, String>> = OnceLock::new();

    AVATAR_URL_CACHE.get_or_init(|| {
        Cache::builder()
            .max_capacity(256)
            .time_to_live(Duration::from_secs(60 * 60))
            .build()
    })
}

#[derive(Clone, Copy, Debug)]
struct CachedRepoMetadata {
    metadata: GitHubRepoMetadata,
    updated_at: i64,
}

fn cached_repo_metadata(cache_key: &str) -> Option<CachedRepoMetadata> {
    let conn = match network_cache_connection() {
        Ok(conn) => conn,
        Err(err) => {
            log::warn!("failed to open network cache for github repo metadata read: {err}");
            return None;
        }
    };

    let row = match conn
        .query_row(
            "SELECT metadata, updated_at FROM github_repo_metadata WHERE cache_key = ?1",
            params![cache_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
    {
        Ok(row) => row,
        Err(err) => {
            log::warn!("failed to read github repo metadata disk cache key={cache_key}: {err}");
            return None;
        }
    };

    let Some((value, updated_at)) = row else {
        return None;
    };
    let Some(metadata) = parse_repo_metadata_value(&value) else {
        log::warn!("invalid github repo metadata disk cache value key={cache_key} value={value}");
        return None;
    };

    Some(CachedRepoMetadata {
        metadata,
        updated_at,
    })
}

fn cache_repo_metadata(
    cache_key: &str,
    workspace_id: &str,
    workspace_root: &str,
    repo_slug: &str,
    remote_name: Option<&str>,
    remote_url: Option<&str>,
    metadata: GitHubRepoMetadata,
) {
    let remote_name = normalized_optional_string(remote_name);
    let remote_url = normalized_optional_string(remote_url);
    let Ok(conn) = network_cache_connection() else {
        log::warn!("failed to open network cache for github repo metadata write");
        return;
    };

    match conn.execute(
        "INSERT INTO github_repo_metadata (
            cache_key,
            workspace_id,
            workspace_root,
            repo_slug,
            remote_name,
            remote_url,
            metadata,
            updated_at
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(cache_key) DO UPDATE SET
            workspace_id = excluded.workspace_id,
            workspace_root = excluded.workspace_root,
            repo_slug = excluded.repo_slug,
            remote_name = excluded.remote_name,
            remote_url = excluded.remote_url,
            metadata = excluded.metadata,
            updated_at = excluded.updated_at",
        params![
            cache_key,
            workspace_id,
            workspace_root,
            repo_slug,
            remote_name,
            remote_url,
            metadata.cache_value(),
            unix_now_secs(),
        ],
    ) {
        Ok(_) => log::debug!(
            "github repo metadata disk cache write workspace_id={} repo={} remote={}",
            workspace_id,
            repo_slug,
            remote_url.as_deref().unwrap_or_default()
        ),
        Err(err) => {
            log::warn!("failed to write github repo metadata disk cache key={cache_key}: {err}")
        }
    }
}

fn cached_avatar_url(email: &str) -> Option<String> {
    let conn = match network_cache_connection() {
        Ok(conn) => conn,
        Err(err) => {
            log::warn!("failed to open network cache for github avatar url read: {err}");
            return None;
        }
    };

    let row = match conn
        .query_row(
            "SELECT url, updated_at FROM github_avatar_urls WHERE email = ?1",
            params![email],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
    {
        Ok(row) => row,
        Err(err) => {
            log::warn!("failed to read github avatar url disk cache email={email}: {err}");
            return None;
        }
    };

    let Some((url, updated_at)) = row else {
        return None;
    };
    if cache_expired(updated_at, GITHUB_AVATAR_URL_CACHE_TTL_SECS) {
        log::debug!(
            "github avatar url disk cache expired email={} age_secs={}",
            email,
            cache_age_secs(updated_at)
        );
        return None;
    }

    log::debug!(
        "github avatar url disk cache hit email={} age_secs={}",
        email,
        cache_age_secs(updated_at)
    );
    Some(url)
}

fn cache_avatar_url(email: &str, url: &str) {
    let Ok(conn) = network_cache_connection() else {
        log::warn!("failed to open network cache for github avatar url write");
        return;
    };

    match conn.execute(
        "INSERT INTO github_avatar_urls (email, url, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(email) DO UPDATE SET
            url = excluded.url,
            updated_at = excluded.updated_at",
        params![email, url, unix_now_secs()],
    ) {
        Ok(_) => log::debug!("github avatar url disk cache write email={email} url={url}"),
        Err(err) => log::warn!("failed to write github avatar url disk cache email={email}: {err}"),
    }
}

fn cached_avatar_bytes_row(cache_key: &str) -> Option<(Vec<u8>, i64)> {
    let conn = match network_cache_connection() {
        Ok(conn) => conn,
        Err(err) => {
            log::warn!("failed to open network cache for github avatar read: {err}");
            return None;
        }
    };

    match conn
        .query_row(
            "SELECT bytes, updated_at FROM github_avatar_cache WHERE cache_key = ?1",
            params![cache_key],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
    {
        Ok(row) => row,
        Err(err) => {
            log::warn!("failed to read github avatar disk cache key={cache_key}: {err}");
            None
        }
    }
}

fn network_cache_connection() -> Result<Connection, rusqlite::Error> {
    let path = network_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS github_repo_metadata (
            cache_key TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            workspace_root TEXT NOT NULL,
            repo_slug TEXT NOT NULL,
            remote_name TEXT,
            remote_url TEXT,
            metadata TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS github_avatar_urls (
            email TEXT PRIMARY KEY,
            url TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS github_avatar_cache (
            cache_key TEXT PRIMARY KEY,
            bytes BLOB NOT NULL,
            updated_at INTEGER NOT NULL
        );",
    )?;
    Ok(conn)
}

fn network_cache_path() -> PathBuf {
    crate::config::craic_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("craic"))
        .join("network-cache.sqlite")
}

fn repo_metadata_cache_key(repo_slug: &str) -> String {
    format!("github:repo:{}", repo_slug.trim())
}

fn normalized_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn cache_expired(updated_at: i64, ttl_secs: i64) -> bool {
    cache_age_secs(updated_at) > ttl_secs
}

fn cache_age_secs(updated_at: i64) -> i64 {
    let now = unix_now_secs();
    if now > updated_at {
        now - updated_at
    } else {
        0
    }
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn percent_encode_path_segment(value: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                let _ = write!(&mut encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}
