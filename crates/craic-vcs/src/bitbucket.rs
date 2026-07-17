use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitbucketRepoMetadata {
    Fork,
    Private,
    Public,
}

impl BitbucketRepoMetadata {
    fn cache_value(self) -> &'static str {
        match self {
            Self::Fork => "fork",
            Self::Private => "private",
            Self::Public => "public",
        }
    }
}

#[derive(Clone, Debug)]
struct ParsedBitbucketRemote {
    host: String,
    slug: String,
}

#[derive(Debug, Deserialize)]
struct BitbucketRepository {
    is_private: Option<bool>,
    parent: Option<Value>,
}

pub fn parse_bitbucket_url(url: &str) -> Option<String> {
    parse_bitbucket_remote(url).map(|repo| repo.slug)
}

pub fn fetch_repo_metadata(remote_url: &str) -> Result<BitbucketRepoMetadata, String> {
    let remote = parse_bitbucket_remote(remote_url).ok_or_else(|| {
        format!(
            "Invalid Bitbucket remote URL. Unable to determine host and project for {remote_url}"
        )
    })?;
    let mut slug_parts = remote.slug.split('/');
    let encoded_owner = slug_parts
        .next()
        .filter(|owner| !owner.is_empty())
        .map(percent_encode_path_segment);
    let encoded_repo = slug_parts
        .next()
        .filter(|repo| !repo.is_empty())
        .map(percent_encode_path_segment);
    let (encoded_owner, encoded_repo) = match (encoded_owner, encoded_repo) {
        (Some(encoded_owner), Some(encoded_repo)) => (encoded_owner, encoded_repo),
        _ => {
            return Err(format!(
                "Invalid Bitbucket repo slug extracted from remote URL: {remote_url}"
            ));
        }
    };
    let api_host = api_host_for_bitbucket(&remote.host);
    let api_url = format!("https://{api_host}/2.0/repositories/{encoded_owner}/{encoded_repo}");

    log::debug!(
        "querying bitbucket repo metadata remote={} api={api_url}",
        remote_url
    );

    let response = Client::new()
        .get(api_url)
        .header("User-Agent", "craic")
        .send()
        .map_err(|err| format!("Failed to query Bitbucket API for {remote_url}: {err}"))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|err| format!("Failed to read Bitbucket API response for {remote_url}: {err}"))?;

    if !status.is_success() {
        let body = body.trim();
        return Err(if body.is_empty() {
            format!("Bitbucket API request failed for {remote_url} with status {status}")
        } else {
            format!("Bitbucket API request for {remote_url} failed ({status}): {body}")
        });
    }

    let repo: BitbucketRepository = serde_json::from_str(&body).map_err(|err| {
        format!("Failed to parse Bitbucket metadata response for {remote_url}: {err}")
    })?;

    parse_bitbucket_repo_metadata_value(&repo)
        .ok_or_else(|| format!("Unexpected Bitbucket metadata payload for {remote_url}: {body}"))
}

pub fn repo_metadata_for_workspace<F>(
    workspace_id: &str,
    workspace_root: &str,
    repo_slug: &str,
    remote_name: Option<&str>,
    remote_url: Option<&str>,
    fetch: F,
) -> Result<BitbucketRepoMetadata, String>
where
    F: FnOnce() -> Result<BitbucketRepoMetadata, String>,
{
    let workspace_id = workspace_id.trim();
    let workspace_root = workspace_root.trim();
    let repo_slug = repo_slug.trim();
    let cache_key = repo_metadata_cache_key(remote_url, repo_slug);

    if let Some(cached) = cached_repo_metadata(&cache_key) {
        log::debug!(
            "bitbucket repo metadata disk cache hit workspace_id={} repo={} remote={} age_secs={}",
            workspace_id,
            repo_slug,
            remote_url.unwrap_or_default(),
            cache_age_secs(cached.updated_at)
        );
        return Ok(cached.metadata);
    }

    log::debug!(
        "bitbucket repo metadata disk cache miss workspace_id={} repo={} remote={}",
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
                    "bitbucket repo metadata fetch failed; using stale cache workspace_id={} repo={} age_secs={} err={}",
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

fn parse_bitbucket_remote(url: &str) -> Option<ParsedBitbucketRemote> {
    let (host, path) = split_git_remote_host_and_path(url)?;
    let host = host.trim().to_ascii_lowercase();

    if !host.contains("bitbucket") {
        return None;
    }

    let path = path
        .trim()
        .trim_start_matches('/')
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.as_str())
        .to_string();

    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }

    let slug = format!("{}/{}", parts[0], parts[1]);
    if slug.is_empty() {
        return None;
    }

    Some(ParsedBitbucketRemote { host, slug })
}

fn api_host_for_bitbucket(host: &str) -> String {
    if host == "bitbucket.org" {
        "api.bitbucket.org".to_string()
    } else {
        host.to_string()
    }
}

fn split_git_remote_host_and_path(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    if let Some((_, tail)) = url.split_once("://") {
        let host_part = tail.split_once('/').map(|(host, _)| host)?;
        let path = tail.splitn(2, '/').nth(1).unwrap_or("");
        let host = host_part
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(host_part);
        return Some((host.to_string(), path.to_string()));
    }

    let host_path = if let Some((_, path)) = url.rsplit_once('@') {
        path
    } else {
        url
    };
    let (host, path) = host_path.split_once(':')?;
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host.to_string(), path.to_string()))
}

fn parse_bitbucket_repo_metadata_value(
    repo: &BitbucketRepository,
) -> Option<BitbucketRepoMetadata> {
    if repo.parent.as_ref().is_some_and(|parent| !parent.is_null()) {
        return Some(BitbucketRepoMetadata::Fork);
    }

    match repo.is_private {
        Some(true) => Some(BitbucketRepoMetadata::Private),
        Some(false) => Some(BitbucketRepoMetadata::Public),
        None => None,
    }
}

#[derive(Clone, Copy, Debug)]
struct CachedRepoMetadata {
    metadata: BitbucketRepoMetadata,
    updated_at: i64,
}

fn cached_repo_metadata(cache_key: &str) -> Option<CachedRepoMetadata> {
    let conn = match network_cache_connection() {
        Ok(conn) => conn,
        Err(err) => {
            log::warn!("failed to open network cache for bitbucket repo metadata read: {err}");
            return None;
        }
    };

    let row = match conn
        .query_row(
            "SELECT metadata, updated_at FROM bitbucket_repo_metadata WHERE cache_key = ?1",
            params![cache_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
    {
        Ok(row) => row,
        Err(err) => {
            log::warn!("failed to read bitbucket repo metadata cache key={cache_key}: {err}");
            return None;
        }
    };

    let Some((value, updated_at)) = row else {
        return None;
    };
    let metadata = match value.as_str() {
        "fork" => BitbucketRepoMetadata::Fork,
        "private" => BitbucketRepoMetadata::Private,
        "public" => BitbucketRepoMetadata::Public,
        _ => return None,
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
    metadata: BitbucketRepoMetadata,
) {
    let conn = match network_cache_connection() {
        Ok(conn) => conn,
        Err(err) => {
            log::warn!("failed to open network cache for bitbucket repo metadata write: {err}");
            return;
        }
    };

    let remote_host = remote_url
        .and_then(parse_bitbucket_remote)
        .map(|remote| remote.host)
        .unwrap_or_default();
    match conn.execute(
        "INSERT INTO bitbucket_repo_metadata (
            cache_key,
            workspace_id,
            workspace_root,
            repo_slug,
            repo_host,
            remote_name,
            remote_url,
            metadata,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(cache_key) DO UPDATE SET
            workspace_id=excluded.workspace_id,
            workspace_root=excluded.workspace_root,
            repo_slug=excluded.repo_slug,
            repo_host=excluded.repo_host,
            remote_name=excluded.remote_name,
            remote_url=excluded.remote_url,
            metadata=excluded.metadata,
            updated_at=excluded.updated_at",
        params![
            cache_key,
            workspace_id,
            workspace_root,
            repo_slug,
            remote_host,
            normalized_optional_string(remote_name),
            normalized_optional_string(remote_url),
            metadata.cache_value(),
            unix_now_secs(),
        ],
    ) {
        Ok(_) => log::debug!(
            "wrote bitbucket repo metadata cache workspace_id={} repo_slug={} metadata={}",
            workspace_id,
            repo_slug,
            metadata.cache_value()
        ),
        Err(err) => log::warn!(
            "failed to cache bitbucket repo metadata workspace_id={} repo_slug={} err={}",
            workspace_id,
            repo_slug,
            err
        ),
    }
}

fn network_cache_connection() -> Result<Connection, rusqlite::Error> {
    let path = network_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS bitbucket_repo_metadata (
            cache_key TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            workspace_root TEXT NOT NULL,
            repo_slug TEXT NOT NULL,
            repo_host TEXT NOT NULL,
            remote_name TEXT,
            remote_url TEXT,
            metadata TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );",
    )?;
    Ok(conn)
}

fn network_cache_path() -> std::path::PathBuf {
    crate::config::craic_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("craic"))
        .join("network-cache.sqlite")
}

fn repo_metadata_cache_key(remote_url: Option<&str>, repo_slug: &str) -> String {
    let host = remote_url
        .and_then(parse_bitbucket_remote)
        .map(|remote| remote.host)
        .unwrap_or_else(|| "bitbucket".to_string());
    format!("{host}:repo:{}", repo_slug.trim())
}

fn normalized_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
