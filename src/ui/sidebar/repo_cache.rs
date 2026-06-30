use moka::sync::Cache;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RepoIconKind {
    Private,
    Public,
    Fork,
    Git,
    Folder,
}

impl RepoIconKind {
    pub(super) fn icon_name(self) -> &'static str {
        match self {
            RepoIconKind::Private => "padlock2-symbolic",
            RepoIconKind::Fork => "branch-fork-symbolic",
            RepoIconKind::Public => "earth-symbolic",
            RepoIconKind::Git => "folder-git-symbolic",
            RepoIconKind::Folder => "folder-symbolic",
        }
    }

    fn cache_value(self) -> &'static str {
        match self {
            RepoIconKind::Private => "private",
            RepoIconKind::Public => "public",
            RepoIconKind::Fork => "fork",
            RepoIconKind::Git => "git",
            RepoIconKind::Folder => "folder",
        }
    }
}

pub(super) fn kind_from_metadata(metadata: crate::git::RepoMetadata) -> RepoIconKind {
    match metadata {
        crate::git::RepoMetadata::Fork => RepoIconKind::Fork,
        crate::git::RepoMetadata::Private => RepoIconKind::Private,
        crate::git::RepoMetadata::Public => RepoIconKind::Public,
        crate::git::RepoMetadata::Folder => RepoIconKind::Folder,
    }
}

pub(super) fn cached_repo_icon_kind(workspace_key: &str) -> Option<RepoIconKind> {
    repo_icon_cache()
        .get(workspace_key)
        .or_else(|| repo_icon_disk_cache_get(workspace_key))
}

pub(super) fn cache_repo_icon_kind(workspace_key: String, kind: RepoIconKind) {
    repo_icon_cache().insert(workspace_key.clone(), kind);
    repo_icon_disk_cache_set(&workspace_key, kind);
}

fn repo_icon_kind_from_cache_value(value: &str) -> Option<RepoIconKind> {
    match value {
        "private" => Some(RepoIconKind::Private),
        "public" => Some(RepoIconKind::Public),
        "fork" => Some(RepoIconKind::Fork),
        "git" => Some(RepoIconKind::Git),
        "folder" => Some(RepoIconKind::Folder),
        _ => None,
    }
}

static REPO_ICON_CACHE: OnceLock<Cache<String, RepoIconKind>> = OnceLock::new();

fn repo_icon_cache() -> &'static Cache<String, RepoIconKind> {
    REPO_ICON_CACHE.get_or_init(|| {
        Cache::builder()
            .max_capacity(512)
            .time_to_live(Duration::from_secs(60 * 60))
            .build()
    })
}

fn repo_icon_disk_cache_get(workspace_key: &str) -> Option<RepoIconKind> {
    let conn = repo_icon_cache_connection().ok()?;
    let kind: String = conn
        .query_row(
            "SELECT kind FROM repo_properties WHERE path = ?1",
            params![workspace_key],
            |row| row.get(0),
        )
        .ok()?;

    let kind = repo_icon_kind_from_cache_value(&kind)?;
    repo_icon_cache().insert(workspace_key.to_string(), kind);
    Some(kind)
}

fn repo_icon_disk_cache_set(workspace_key: &str, kind: RepoIconKind) {
    let Ok(conn) = repo_icon_cache_connection() else {
        return;
    };

    let _ = conn.execute(
        "INSERT INTO repo_properties (path, kind, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET
            kind = excluded.kind,
            updated_at = excluded.updated_at",
        params![workspace_key, kind.cache_value(), unix_now_secs(),],
    );
}

fn repo_icon_cache_connection() -> Result<Connection, rusqlite::Error> {
    let path = repo_icon_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let conn = Connection::open(path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS repo_properties (
            path TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        )",
        [],
    )?;
    Ok(conn)
}

fn repo_icon_cache_path() -> PathBuf {
    if let Some(cache_home) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(cache_home)
            .join("craic")
            .join("repo-properties.sqlite3");
    }

    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("craic")
            .join("repo-properties.sqlite3");
    }

    std::env::temp_dir()
        .join("craic")
        .join("repo-properties.sqlite3")
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}
