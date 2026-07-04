use moka::sync::Cache;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RepoIconKind {
    Private,
    Public,
    Fork,
    Local,
    Unknown,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RepoProperties {
    pub(super) kind: RepoIconKind,
    pub(super) remote_label: Option<String>,
}

impl RepoIconKind {
    pub(super) fn icon_name(self) -> &'static str {
        match self {
            RepoIconKind::Private => "padlock2-symbolic",
            RepoIconKind::Fork => "branch-fork-symbolic",
            RepoIconKind::Public => "earth-symbolic",
            RepoIconKind::Local => "folder-git-symbolic",
            RepoIconKind::Unknown => "dialog-question-symbolic",
            RepoIconKind::Folder => "folder-symbolic",
        }
    }

    pub(super) fn is_remote_metadata(self) -> bool {
        matches!(self, Self::Private | Self::Public | Self::Fork)
    }

    fn cache_value(self) -> &'static str {
        match self {
            RepoIconKind::Private => "private",
            RepoIconKind::Public => "public",
            RepoIconKind::Fork => "fork",
            RepoIconKind::Local => "local",
            RepoIconKind::Unknown => "unknown",
            RepoIconKind::Folder => "folder",
        }
    }
}

pub(super) fn kind_from_metadata(metadata: crate::git::RepoMetadata) -> RepoIconKind {
    match metadata {
        crate::git::RepoMetadata::Fork => RepoIconKind::Fork,
        crate::git::RepoMetadata::Private => RepoIconKind::Private,
        crate::git::RepoMetadata::Public => RepoIconKind::Public,
        crate::git::RepoMetadata::Local => RepoIconKind::Local,
        crate::git::RepoMetadata::Unknown => RepoIconKind::Unknown,
        crate::git::RepoMetadata::Folder => RepoIconKind::Folder,
    }
}

pub(super) fn cached_repo_icon_kind(workspace_key: &str) -> Option<RepoIconKind> {
    cached_repo_properties(workspace_key).map(|properties| properties.kind)
}

pub(super) fn cached_repo_properties(workspace_key: &str) -> Option<RepoProperties> {
    repo_properties_cache()
        .get(workspace_key)
        .or_else(|| repo_properties_disk_cache_get(workspace_key))
}

pub(super) fn cache_repo_properties(workspace_key: String, properties: RepoProperties) {
    repo_properties_cache().insert(workspace_key.clone(), properties.clone());
    repo_properties_disk_cache_set(&workspace_key, &properties);
}

pub(super) fn cache_resolved_repo_properties(
    workspace_key: String,
    properties: RepoProperties,
) -> RepoProperties {
    let mut properties = properties;
    if let Some(cached) = cached_repo_properties(&workspace_key)
        && cached.kind.is_remote_metadata()
        && properties.kind == RepoIconKind::Unknown
        && properties.remote_label.is_some()
        && (cached.remote_label.is_none() || properties.remote_label == cached.remote_label)
    {
        log::debug!(
            "repo metadata refresh preserving cached remote kind workspace={} cached={:?} resolved={:?}",
            workspace_key,
            cached.kind,
            properties.kind
        );
        properties.kind = cached.kind;
    }

    cache_repo_properties(workspace_key, properties.clone());
    properties
}

fn repo_icon_kind_from_cache_value(value: &str) -> Option<RepoIconKind> {
    match value {
        "private" => Some(RepoIconKind::Private),
        "public" => Some(RepoIconKind::Public),
        "fork" => Some(RepoIconKind::Fork),
        "local" => Some(RepoIconKind::Local),
        "unknown" => Some(RepoIconKind::Unknown),
        "folder" => Some(RepoIconKind::Folder),
        _ => None,
    }
}

static REPO_PROPERTIES_CACHE: OnceLock<Cache<String, RepoProperties>> = OnceLock::new();

fn repo_properties_cache() -> &'static Cache<String, RepoProperties> {
    REPO_PROPERTIES_CACHE.get_or_init(|| Cache::builder().max_capacity(512).build())
}

fn repo_properties_disk_cache_get(workspace_key: &str) -> Option<RepoProperties> {
    let conn = repo_properties_cache_connection().ok()?;
    let (kind, remote_label): (String, Option<String>) = conn
        .query_row(
            "SELECT kind, remote_label FROM repo_properties WHERE path = ?1",
            params![workspace_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;

    let kind = repo_icon_kind_from_cache_value(&kind)?;
    let properties = RepoProperties { kind, remote_label };
    repo_properties_cache().insert(workspace_key.to_string(), properties.clone());
    Some(properties)
}

fn repo_properties_disk_cache_set(workspace_key: &str, properties: &RepoProperties) {
    let Ok(conn) = repo_properties_cache_connection() else {
        return;
    };

    let _ = conn.execute(
        "INSERT INTO repo_properties (path, kind, remote_label, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET
            kind = excluded.kind,
            remote_label = excluded.remote_label,
            updated_at = excluded.updated_at",
        params![
            workspace_key,
            properties.kind.cache_value(),
            properties.remote_label.as_deref(),
            unix_now_secs(),
        ],
    );
}

fn repo_properties_cache_connection() -> Result<Connection, rusqlite::Error> {
    let path = repo_properties_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let conn = Connection::open(path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS repo_properties (
            path TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            remote_label TEXT,
            updated_at INTEGER NOT NULL
        )",
        [],
    )?;
    ensure_repo_properties_remote_label_column(&conn)?;
    Ok(conn)
}

fn ensure_repo_properties_remote_label_column(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut statement = conn.prepare("PRAGMA table_info(repo_properties)")?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for column in columns {
        if column? == "remote_label" {
            return Ok(());
        }
    }

    conn.execute(
        "ALTER TABLE repo_properties ADD COLUMN remote_label TEXT",
        [],
    )?;
    Ok(())
}

fn repo_properties_cache_path() -> PathBuf {
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
