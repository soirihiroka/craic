mod native;
pub use native::{NativeHistory, list_native_sessions};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DB_BUSY_TIMEOUT_MS: i32 = 3000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceKey {
    key: String,
    git_remote_url: Option<String>,
    repo_path: PathBuf,
}

impl WorkspaceKey {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn git_remote_url(&self) -> Option<&str> {
        self.git_remote_url.as_deref()
    }

    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreState {
    Unmapped,
    Restorable,
    Unsupported,
    Ambiguous,
    Missing,
}

impl RestoreState {
    pub fn as_str(self) -> &'static str {
        match self {
            RestoreState::Unmapped => "unmapped",
            RestoreState::Restorable => "restorable",
            RestoreState::Unsupported => "unsupported",
            RestoreState::Ambiguous => "ambiguous",
            RestoreState::Missing => "missing",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "restorable" => RestoreState::Restorable,
            "unsupported" => RestoreState::Unsupported,
            "ambiguous" => RestoreState::Ambiguous,
            "missing" => RestoreState::Missing,
            _ => RestoreState::Unmapped,
        }
    }

    pub fn is_restorable(self) -> bool {
        self == RestoreState::Restorable
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSessionRow {
    pub id: i64,
    pub session_uuid: String,
    pub provider_id: String,
    pub workspace_key: String,
    pub git_remote_url: Option<String>,
    pub repo_path: PathBuf,
    pub title: String,
    pub normalized_title: String,
    pub task_description: Option<String>,
    pub normalized_task_description: Option<String>,
    pub cli_session_id: Option<String>,
    pub restore_state: RestoreState,
    pub metadata_json: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSessionSummary {
    pub task_description: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexThreadOverlay {
    pub thread_id: String,
    pub workspace_key: String,
    pub task_description: Option<String>,
    pub normalized_task_description: Option<String>,
    pub tags: Vec<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexThreadOverlayUpsert {
    pub thread_id: String,
    pub workspace_key: String,
    pub task_description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceTag {
    pub tag: String,
    pub session_count: usize,
}

pub fn workspace_for_system_path(
    workspace_key: impl Into<String>,
    target_root: impl Into<String>,
) -> WorkspaceKey {
    let target_root = target_root.into();
    WorkspaceKey {
        key: workspace_key.into(),
        git_remote_url: None,
        repo_path: PathBuf::from(target_root),
    }
}

pub fn normalize_title(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn initialize_history_database() -> Result<(), String> {
    history_connection().map(|_| ()).map_err(db_error)
}

pub fn list_sessions(
    workspace_key: &str,
    limit: usize,
    offset: usize,
    title_query: Option<&str>,
    tag_filters: &[String],
) -> Result<Vec<AgentSessionRow>, String> {
    let conn = history_read_connection().map_err(db_error)?;
    let normalized_query = title_query
        .map(normalize_title_for_match)
        .filter(|query| !query.is_empty());
    let normalized_tags = tag_filters
        .iter()
        .map(|tag| normalize_tag(tag))
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    let base_select = "SELECT id, session_uuid, provider, workspace_key, git_remote_url, repo_path,
                              title, normalized_title, task_description,
                              normalized_task_description, cli_session_id, restore_state,
                              metadata_json, created_at_ms, updated_at_ms, last_seen_at_ms,
                              ended_at_ms
                       FROM agent_sessions";
    let mut clauses = vec!["workspace_key = ?".to_string()];
    let mut values = vec![SqlValue::from(workspace_key.to_string())];

    if !normalized_tags.is_empty() {
        let placeholders = std::iter::repeat_n("?", normalized_tags.len())
            .collect::<Vec<_>>()
            .join(", ");
        clauses.push(format!(
            "EXISTS (
                SELECT 1 FROM agent_session_tags tag
                WHERE tag.session_id = agent_sessions.id
                  AND tag.normalized_tag IN ({placeholders})
             )"
        ));
        values.extend(normalized_tags.into_iter().map(SqlValue::from));
    }

    if let Some(query) = normalized_query {
        let pattern = format!("%{}%", escape_like_pattern(&query));
        clauses.push(
            "(normalized_title LIKE ? ESCAPE '\\'
              OR COALESCE(normalized_task_description, '') LIKE ? ESCAPE '\\')"
                .to_string(),
        );
        values.push(SqlValue::from(pattern.clone()));
        values.push(SqlValue::from(pattern));
    }

    values.push(SqlValue::from(limit as i64));
    values.push(SqlValue::from(offset as i64));

    let mut stmt = conn
        .prepare(&format!(
            "{base_select}
             WHERE {}
             ORDER BY last_seen_at_ms DESC, id DESC
             LIMIT ? OFFSET ?",
            clauses.join(" AND ")
        ))
        .map_err(db_error)?;
    let rows = stmt
        .query_map(params_from_iter(values), row_from_db)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(rows)
}

pub fn lookup_session(local_id: i64) -> Result<Option<AgentSessionRow>, String> {
    let conn = history_connection().map_err(db_error)?;
    conn.query_row(
        "SELECT id, session_uuid, provider, workspace_key, git_remote_url, repo_path, title,
                normalized_title, task_description, normalized_task_description,
                cli_session_id, restore_state, metadata_json,
                created_at_ms, updated_at_ms, last_seen_at_ms, ended_at_ms
         FROM agent_sessions
         WHERE id = ?1",
        params![local_id],
        row_from_db,
    )
    .optional()
    .map_err(db_error)
}

pub fn update_session_summary(local_id: i64, summary: &AgentSessionSummary) -> Result<(), String> {
    let description = normalize_title(&summary.task_description);
    if description.is_empty() {
        return Err("agent session task description is empty".to_string());
    }
    let tags = normalized_summary_tags(&summary.tags);
    let conn = history_connection().map_err(db_error)?;
    let now = unix_now_ms();
    let tx = conn.unchecked_transaction().map_err(db_error)?;
    let updated = tx
        .execute(
            "UPDATE agent_sessions
             SET task_description = ?2,
                 normalized_task_description = ?3,
                 updated_at_ms = ?4
             WHERE id = ?1",
            params![
                local_id,
                description,
                normalize_title_for_match(&description),
                now
            ],
        )
        .map_err(db_error)?;
    if updated == 0 {
        return Err(format!("Agent history session {local_id} was not found."));
    }
    tx.execute(
        "DELETE FROM agent_session_tags WHERE session_id = ?1",
        params![local_id],
    )
    .map_err(db_error)?;
    for tag in tags {
        tx.execute(
            "INSERT OR IGNORE INTO agent_session_tags (session_id, tag, normalized_tag)
             VALUES (?1, ?2, ?3)",
            params![local_id, tag, normalize_tag(&tag)],
        )
        .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;
    log::info!(
        "agent history summary saved local_id={} description_bytes={} tags={}",
        local_id,
        description.len(),
        summary.tags.len()
    );
    Ok(())
}

pub fn upsert_codex_thread_overlay(
    input: CodexThreadOverlayUpsert,
) -> Result<CodexThreadOverlay, String> {
    let thread_id = input.thread_id.trim();
    if thread_id.is_empty() {
        return Err("Codex thread ID is empty.".to_string());
    }
    let workspace_key = input.workspace_key.trim();
    if workspace_key.is_empty() {
        return Err("Codex thread workspace key is empty.".to_string());
    }

    let task_description = input
        .task_description
        .as_deref()
        .map(normalize_title)
        .filter(|description| !description.is_empty());
    let normalized_task_description = task_description.as_deref().map(normalize_title_for_match);
    let tags = normalized_summary_tags(&input.tags);
    let now = unix_now_ms();
    let conn = history_connection().map_err(db_error)?;
    let tx = conn.unchecked_transaction().map_err(db_error)?;
    tx.execute(
        "INSERT INTO codex_thread_overlays (
            workspace_key, thread_id, task_description, normalized_task_description,
            created_at_ms, updated_at_ms
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(workspace_key, thread_id) DO UPDATE SET
            task_description = excluded.task_description,
            normalized_task_description = excluded.normalized_task_description,
            updated_at_ms = excluded.updated_at_ms",
        params![
            workspace_key,
            thread_id,
            task_description,
            normalized_task_description,
            now
        ],
    )
    .map_err(db_error)?;
    tx.execute(
        "DELETE FROM codex_thread_tags WHERE workspace_key = ?1 AND thread_id = ?2",
        params![workspace_key, thread_id],
    )
    .map_err(db_error)?;
    for tag in &tags {
        tx.execute(
            "INSERT INTO codex_thread_tags (workspace_key, thread_id, tag, normalized_tag)
             VALUES (?1, ?2, ?3, ?4)",
            params![workspace_key, thread_id, tag, normalize_tag(tag)],
        )
        .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;

    log::info!(
        "Codex thread overlay saved workspace={} thread_id={} description_present={} tags={}",
        workspace_key,
        thread_id,
        task_description.is_some(),
        tags.len()
    );
    lookup_codex_thread_overlay(workspace_key, thread_id)?
        .ok_or_else(|| "Codex thread overlay was not readable after upsert.".to_string())
}

pub fn lookup_codex_thread_overlay(
    workspace_key: &str,
    thread_id: &str,
) -> Result<Option<CodexThreadOverlay>, String> {
    let conn = history_connection().map_err(db_error)?;
    let overlay = conn
        .query_row(
            "SELECT thread_id, workspace_key, task_description,
                    normalized_task_description, created_at_ms, updated_at_ms
             FROM codex_thread_overlays
             WHERE workspace_key = ?1 AND thread_id = ?2",
            params![workspace_key, thread_id],
            codex_thread_overlay_from_db,
        )
        .optional()
        .map_err(db_error)?;
    overlay
        .map(|overlay| load_codex_thread_overlay_tags(&conn, overlay))
        .transpose()
}

pub fn update_codex_thread_overlay_tags(
    workspace_key: &str,
    thread_id: &str,
    tags: &[String],
) -> Result<CodexThreadOverlay, String> {
    let workspace_key = workspace_key.trim();
    if workspace_key.is_empty() {
        return Err("Codex thread workspace key is empty.".to_string());
    }
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err("Codex thread ID is empty.".to_string());
    }

    let tags = normalized_summary_tags(tags);
    let now = unix_now_ms();
    let conn = history_connection().map_err(db_error)?;
    let tx = conn.unchecked_transaction().map_err(db_error)?;
    tx.execute(
        "INSERT INTO codex_thread_overlays (
            workspace_key, thread_id, task_description, normalized_task_description,
            created_at_ms, updated_at_ms
         )
         VALUES (?1, ?2, NULL, NULL, ?3, ?3)
         ON CONFLICT(workspace_key, thread_id) DO UPDATE SET
            updated_at_ms = excluded.updated_at_ms",
        params![workspace_key, thread_id, now],
    )
    .map_err(db_error)?;
    tx.execute(
        "DELETE FROM codex_thread_tags WHERE workspace_key = ?1 AND thread_id = ?2",
        params![workspace_key, thread_id],
    )
    .map_err(db_error)?;
    for tag in &tags {
        tx.execute(
            "INSERT INTO codex_thread_tags (workspace_key, thread_id, tag, normalized_tag)
             VALUES (?1, ?2, ?3, ?4)",
            params![workspace_key, thread_id, tag, normalize_tag(tag)],
        )
        .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;

    log::info!(
        "Codex thread tags updated workspace={} thread_id={} tags={}",
        workspace_key,
        thread_id,
        tags.len()
    );
    lookup_codex_thread_overlay(workspace_key, thread_id)?
        .ok_or_else(|| "Codex thread overlay was not readable after updating tags.".to_string())
}

pub fn list_codex_thread_overlays(
    workspace_key: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<CodexThreadOverlay>, String> {
    let conn = history_connection().map_err(db_error)?;
    let mut stmt = conn
        .prepare(
            "SELECT thread_id, workspace_key, task_description,
                    normalized_task_description, created_at_ms, updated_at_ms
             FROM codex_thread_overlays
             WHERE workspace_key = ?1
             ORDER BY updated_at_ms DESC, thread_id
             LIMIT ?2 OFFSET ?3",
        )
        .map_err(db_error)?;
    let overlays = stmt
        .query_map(
            params![workspace_key, limit as i64, offset as i64],
            codex_thread_overlay_from_db,
        )
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    overlays
        .into_iter()
        .map(|overlay| load_codex_thread_overlay_tags(&conn, overlay))
        .collect()
}

pub fn delete_codex_thread_overlay(workspace_key: &str, thread_id: &str) -> Result<(), String> {
    let conn = history_connection().map_err(db_error)?;
    let tx = conn.unchecked_transaction().map_err(db_error)?;
    tx.execute(
        "DELETE FROM codex_thread_tags WHERE workspace_key = ?1 AND thread_id = ?2",
        params![workspace_key, thread_id],
    )
    .map_err(db_error)?;
    let deleted = tx
        .execute(
            "DELETE FROM codex_thread_overlays WHERE workspace_key = ?1 AND thread_id = ?2",
            params![workspace_key, thread_id],
        )
        .map_err(db_error)?;
    if deleted == 0 {
        return Err(format!(
            "Codex thread overlay {thread_id} was not found in workspace {workspace_key}."
        ));
    }
    tx.commit().map_err(db_error)?;
    log::info!(
        "Codex thread overlay deleted workspace={} thread_id={}",
        workspace_key,
        thread_id
    );
    Ok(())
}

pub fn workspace_tags(workspace_key: &str) -> Result<Vec<String>, String> {
    let conn = history_connection().map_err(db_error)?;
    let mut stmt = conn
        .prepare(
            "SELECT tag.tag
             FROM agent_session_tags tag
             JOIN agent_sessions session ON session.id = tag.session_id
             WHERE session.workspace_key = ?1
             GROUP BY tag.normalized_tag
             ORDER BY lower(tag.tag), tag.tag",
        )
        .map_err(db_error)?;
    stmt.query_map(params![workspace_key], |row| row.get(0))
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)
}

pub fn workspace_tag_counts(workspace_key: &str) -> Result<Vec<WorkspaceTag>, String> {
    let conn = history_read_connection().map_err(db_error)?;
    let mut stmt = conn
        .prepare(
            "SELECT MIN(tag.tag), COUNT(*)
             FROM agent_session_tags tag
             JOIN agent_sessions session ON session.id = tag.session_id
             WHERE session.workspace_key = ?1
             GROUP BY tag.normalized_tag
             ORDER BY COUNT(*) DESC, lower(MIN(tag.tag)), MIN(tag.tag)",
        )
        .map_err(db_error)?;
    stmt.query_map(params![workspace_key], |row| {
        let count: i64 = row.get(1)?;
        Ok(WorkspaceTag {
            tag: row.get(0)?,
            session_count: usize::try_from(count).unwrap_or(usize::MAX),
        })
    })
    .map_err(db_error)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_error)
}

pub fn max_local_session_id() -> Result<Option<i64>, String> {
    let conn = history_connection().map_err(db_error)?;
    conn.query_row("SELECT MAX(id) FROM agent_sessions", [], |row| row.get(0))
        .map_err(db_error)
}

pub fn mark_ended(local_id: i64) -> Result<(), String> {
    let conn = history_connection().map_err(db_error)?;
    let now = unix_now_ms();
    conn.execute(
        "UPDATE agent_sessions
         SET ended_at_ms = COALESCE(ended_at_ms, ?2),
             updated_at_ms = ?2,
             last_seen_at_ms = ?2
         WHERE id = ?1",
        params![local_id, now],
    )
    .map_err(db_error)?;
    Ok(())
}

pub fn hide_native_session(local_id: i64) -> Result<(), String> {
    let row =
        lookup_session(local_id)?.ok_or_else(|| "Agent session was not found.".to_string())?;
    let mut metadata = serde_json::from_str::<Value>(&row.metadata_json)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    metadata["hidden"] = Value::Bool(true);
    history_connection()
        .map_err(db_error)?
        .execute(
            "UPDATE agent_sessions SET metadata_json = ?2 WHERE id = ?1",
            params![local_id, metadata.to_string()],
        )
        .map_err(db_error)?;
    Ok(())
}

fn history_connection() -> Result<Connection, rusqlite::Error> {
    let Some(path) = crate::config::sessions_db_path() else {
        return Err(rusqlite::Error::InvalidPath(PathBuf::from(
            "HOME is not set",
        )));
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", DB_BUSY_TIMEOUT_MS)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_uuid TEXT,
            provider TEXT NOT NULL,
            workspace_key TEXT NOT NULL,
            git_remote_url TEXT,
            repo_path TEXT NOT NULL,
            title TEXT NOT NULL,
            normalized_title TEXT NOT NULL,
            cli_session_id TEXT,
            task_description TEXT,
            normalized_task_description TEXT,
            restore_state TEXT NOT NULL DEFAULT 'unmapped',
            metadata_json TEXT NOT NULL DEFAULT '{}',
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            last_seen_at_ms INTEGER NOT NULL,
            ended_at_ms INTEGER,
            UNIQUE(provider, workspace_key, normalized_title)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_workspace_seen
            ON agent_sessions(workspace_key, last_seen_at_ms DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_cli_session
            ON agent_sessions(provider, cli_session_id);
        CREATE TABLE IF NOT EXISTS agent_session_tags (
            session_id INTEGER NOT NULL,
            tag TEXT NOT NULL,
            normalized_tag TEXT NOT NULL,
            PRIMARY KEY(session_id, normalized_tag),
            FOREIGN KEY(session_id) REFERENCES agent_sessions(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_agent_session_tags_workspace
            ON agent_session_tags(normalized_tag, session_id);
        CREATE TABLE IF NOT EXISTS codex_thread_overlays (
            workspace_key TEXT NOT NULL,
            thread_id TEXT NOT NULL,
            task_description TEXT,
            normalized_task_description TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY(workspace_key, thread_id)
        );
        CREATE INDEX IF NOT EXISTS idx_codex_thread_overlays_workspace_updated
            ON codex_thread_overlays(workspace_key, updated_at_ms DESC, thread_id);
        CREATE TABLE IF NOT EXISTS codex_thread_tags (
            workspace_key TEXT NOT NULL,
            thread_id TEXT NOT NULL,
            tag TEXT NOT NULL,
            normalized_tag TEXT NOT NULL,
            PRIMARY KEY(workspace_key, thread_id, normalized_tag),
            FOREIGN KEY(workspace_key, thread_id)
                REFERENCES codex_thread_overlays(workspace_key, thread_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_codex_thread_tags_workspace
            ON codex_thread_tags(workspace_key, normalized_tag, thread_id);",
    )?;
    ensure_history_columns(&conn)?;
    Ok(conn)
}

fn history_read_connection() -> Result<Connection, rusqlite::Error> {
    let Some(path) = crate::config::sessions_db_path() else {
        return Err(rusqlite::Error::InvalidPath(PathBuf::from(
            "HOME is not set",
        )));
    };
    if !path.is_file() {
        return history_connection();
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.pragma_update(None, "busy_timeout", DB_BUSY_TIMEOUT_MS)?;
    Ok(conn)
}

fn ensure_history_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
    let columns = conn
        .prepare("PRAGMA table_info(agent_sessions)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<HashSet<_>, _>>()?;
    if !columns.contains("task_description") {
        conn.execute(
            "ALTER TABLE agent_sessions ADD COLUMN task_description TEXT",
            [],
        )?;
    }
    if !columns.contains("normalized_task_description") {
        conn.execute(
            "ALTER TABLE agent_sessions ADD COLUMN normalized_task_description TEXT",
            [],
        )?;
    }
    Ok(())
}

fn row_from_db(row: &rusqlite::Row<'_>) -> Result<AgentSessionRow, rusqlite::Error> {
    let restore_state: String = row.get(11)?;
    let repo_path: String = row.get(5)?;
    Ok(AgentSessionRow {
        id: row.get(0)?,
        session_uuid: row.get(1)?,
        provider_id: row.get(2)?,
        workspace_key: row.get(3)?,
        git_remote_url: row.get(4)?,
        repo_path: PathBuf::from(repo_path),
        title: row.get(6)?,
        normalized_title: row.get(7)?,
        task_description: row.get(8)?,
        normalized_task_description: row.get(9)?,
        cli_session_id: row.get(10)?,
        restore_state: RestoreState::from_str(&restore_state),
        metadata_json: row.get(12)?,
        created_at_ms: row.get(13)?,
        updated_at_ms: row.get(14)?,
        last_seen_at_ms: row.get(15)?,
        ended_at_ms: row.get(16)?,
    })
}

fn codex_thread_overlay_from_db(
    row: &rusqlite::Row<'_>,
) -> Result<CodexThreadOverlay, rusqlite::Error> {
    Ok(CodexThreadOverlay {
        thread_id: row.get(0)?,
        workspace_key: row.get(1)?,
        task_description: row.get(2)?,
        normalized_task_description: row.get(3)?,
        tags: Vec::new(),
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
    })
}

fn load_codex_thread_overlay_tags(
    conn: &Connection,
    mut overlay: CodexThreadOverlay,
) -> Result<CodexThreadOverlay, String> {
    let mut stmt = conn
        .prepare(
            "SELECT tag
             FROM codex_thread_tags
             WHERE workspace_key = ?1 AND thread_id = ?2
             ORDER BY lower(tag), tag",
        )
        .map_err(db_error)?;
    overlay.tags = stmt
        .query_map(params![overlay.workspace_key, overlay.thread_id], |row| {
            row.get(0)
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(overlay)
}

pub fn new_session_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn normalize_title_for_match(text: &str) -> String {
    normalize_title(text).to_ascii_lowercase()
}

fn normalized_summary_tags(tags: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    tags.iter()
        .filter_map(|tag| {
            let normalized = normalize_tag(tag);
            if normalized.is_empty() || !seen.insert(normalized.clone()) {
                None
            } else {
                Some(normalized)
            }
        })
        .take(12)
        .collect()
}

fn normalize_tag(text: &str) -> String {
    normalize_title(text).to_ascii_lowercase()
}

fn escape_like_pattern(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn newest_usable_codex_state_db() -> Option<PathBuf> {
    let codex_dir = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))?;
    let mut paths = fs::read_dir(codex_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("state_") && name.ends_with(".sqlite"))
        })
        .collect::<Vec<_>>();

    paths.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH)
    });
    paths.reverse();

    paths
        .into_iter()
        .find(|path| codex_state_db_is_usable(path))
}

fn codex_state_db_is_usable(path: &Path) -> bool {
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return false;
    };
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'threads'",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

pub fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn db_error(err: rusqlite::Error) -> String {
    err.to_string()
}
