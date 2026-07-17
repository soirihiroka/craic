use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DB_BUSY_TIMEOUT_MS: i32 = 3000;
const DEFAULT_METADATA_JSON: &str = "{}";
const CODEX_TIME_MATCH_WINDOW_MS: i64 = 5 * 60 * 1000;
const CODEX_TIME_MATCH_AMBIGUITY_MS: i64 = 10 * 1000;

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
pub struct WorkspaceTag {
    pub tag: String,
    pub session_count: usize,
}

#[derive(Clone, Debug)]
pub struct AgentSessionUpsert {
    pub provider_id: String,
    pub workspace: WorkspaceKey,
    pub title: String,
    pub initial_restore_state: RestoreState,
    pub session_uuid: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexMappingOutcome {
    Restorable(String),
    Ambiguous,
    Missing,
    Unsupported,
    Skipped,
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

pub fn default_title_should_persist(title: &str) -> bool {
    let title = normalize_title(title);
    let lower = title.to_ascii_lowercase();
    !title.is_empty()
        && !matches!(
            lower.as_str(),
            "new chat" | "new codex chat" | "new agy chat" | "new opencode chat"
        )
}

pub fn upsert_session(input: AgentSessionUpsert) -> Result<AgentSessionRow, String> {
    let title = normalize_title(&input.title);
    if !default_title_should_persist(&title) {
        return Err("default agent session title is not persisted".to_string());
    }

    let normalized_title = normalize_title_for_match(&title);
    let now = unix_now_ms();
    let conn = history_connection().map_err(db_error)?;
    conn.execute(
        "INSERT INTO agent_sessions (
            session_uuid, provider, workspace_key, git_remote_url, repo_path, title,
            normalized_title, restore_state, metadata_json, created_at_ms,
            updated_at_ms, last_seen_at_ms
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10)
        ON CONFLICT(provider, workspace_key, normalized_title) DO UPDATE SET
            git_remote_url = excluded.git_remote_url,
            repo_path = excluded.repo_path,
            title = excluded.title,
            cli_session_id = NULL,
            restore_state = excluded.restore_state,
            metadata_json = excluded.metadata_json,
            updated_at_ms = excluded.updated_at_ms,
            last_seen_at_ms = excluded.last_seen_at_ms,
            ended_at_ms = NULL",
        params![
            input.session_uuid.unwrap_or_else(new_session_uuid),
            input.provider_id,
            input.workspace.key(),
            input.workspace.git_remote_url(),
            input.workspace.repo_path().to_string_lossy(),
            title,
            normalized_title,
            input.initial_restore_state.as_str(),
            DEFAULT_METADATA_JSON,
            now,
        ],
    )
    .map_err(db_error)?;

    lookup_by_title(
        &conn,
        input.workspace.key(),
        &input.provider_id,
        &normalized_title,
    )?
    .ok_or_else(|| "agent session row was not readable after upsert".to_string())
}

pub fn upsert_session_for_manual_id(
    input: AgentSessionUpsert,
    session_id: u64,
) -> Result<AgentSessionRow, String> {
    let title = normalize_title(&input.title);
    if title.is_empty() {
        return Err("agent session title is empty".to_string());
    }

    let normalized_title = if default_title_should_persist(&title) {
        normalize_title_for_match(&title)
    } else {
        format!("{} #{session_id}", normalize_title_for_match(&title))
    };
    let now = unix_now_ms();
    let conn = history_connection().map_err(db_error)?;
    conn.execute(
        "INSERT INTO agent_sessions (
            session_uuid, provider, workspace_key, git_remote_url, repo_path, title,
            normalized_title, restore_state, metadata_json, created_at_ms,
            updated_at_ms, last_seen_at_ms
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10)
        ON CONFLICT(provider, workspace_key, normalized_title) DO UPDATE SET
            git_remote_url = excluded.git_remote_url,
            repo_path = excluded.repo_path,
            title = excluded.title,
            restore_state = excluded.restore_state,
            metadata_json = excluded.metadata_json,
            updated_at_ms = excluded.updated_at_ms,
            last_seen_at_ms = excluded.last_seen_at_ms,
            ended_at_ms = NULL",
        params![
            input.session_uuid.unwrap_or_else(new_session_uuid),
            input.provider_id,
            input.workspace.key(),
            input.workspace.git_remote_url(),
            input.workspace.repo_path().to_string_lossy(),
            title,
            normalized_title,
            input.initial_restore_state.as_str(),
            DEFAULT_METADATA_JSON,
            now,
        ],
    )
    .map_err(db_error)?;

    lookup_by_title(
        &conn,
        input.workspace.key(),
        &input.provider_id,
        &normalized_title,
    )?
    .ok_or_else(|| "agent session row was not readable after upsert".to_string())
}

pub fn list_sessions(
    workspace_key: &str,
    limit: usize,
    offset: usize,
    title_query: Option<&str>,
    tag_filters: &[String],
) -> Result<Vec<AgentSessionRow>, String> {
    let conn = history_connection().map_err(db_error)?;
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
    let conn = history_connection().map_err(db_error)?;
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

pub fn update_mapping(
    local_id: i64,
    cli_session_id: Option<&str>,
    restore_state: RestoreState,
    metadata_json: &str,
) -> Result<(), String> {
    let conn = history_connection().map_err(db_error)?;
    let now = unix_now_ms();
    conn.execute(
        "UPDATE agent_sessions
         SET cli_session_id = ?2,
             restore_state = ?3,
             metadata_json = ?4,
             updated_at_ms = ?5
         WHERE id = ?1",
        params![
            local_id,
            cli_session_id,
            restore_state.as_str(),
            metadata_json,
            now,
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

pub fn update_session_title(local_id: i64, title: &str) -> Result<AgentSessionRow, String> {
    let title = normalize_title(title);
    if !default_title_should_persist(&title) {
        return Err("default agent session title is not persisted".to_string());
    }

    let conn = history_connection().map_err(db_error)?;
    let normalized_title = normalize_title_for_match(&title);
    let now = unix_now_ms();
    conn.execute(
        "UPDATE agent_sessions
         SET title = ?2,
             normalized_title = ?3,
             updated_at_ms = ?4
         WHERE id = ?1",
        params![local_id, title, normalized_title, now],
    )
    .map_err(db_error)?;

    lookup_session(local_id)?
        .ok_or_else(|| format!("Agent history session {local_id} was not found."))
}

pub fn set_manual_session_id(local_id: i64, cli_session_id: &str) -> Result<(), String> {
    let Some(row) = lookup_session(local_id)? else {
        return Err(format!("Agent history session {local_id} was not found."));
    };
    let restore_state = if row.provider_id == "codex" {
        RestoreState::Restorable
    } else {
        RestoreState::Unsupported
    };
    let metadata = json!({
        "manual_cli_session_id": true,
        "previous_restore_state": row.restore_state.as_str(),
    })
    .to_string();
    update_mapping(local_id, Some(cli_session_id), restore_state, &metadata)?;
    log::info!(
        "agent history manual session id set local_id={} provider={} cli_session_id={} restore_state={}",
        local_id,
        row.provider_id,
        cli_session_id,
        restore_state.as_str()
    );
    Ok(())
}

pub fn suggested_cli_session_id(row: &AgentSessionRow) -> Option<String> {
    row.cli_session_id
        .clone()
        .or_else(|| metadata_string(&row.metadata_json, "candidate_cli_session_id"))
}

pub fn cli_session_id_is_empty(local_id: i64) -> Result<bool, String> {
    Ok(lookup_session(local_id)?.is_some_and(|row| {
        row.cli_session_id
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    }))
}

pub fn mark_ended(local_id: i64) -> Result<(), String> {
    let conn = history_connection().map_err(db_error)?;
    let now = unix_now_ms();
    conn.execute(
        "UPDATE agent_sessions
         SET ended_at_ms = COALESCE(ended_at_ms, ?2),
             updated_at_ms = ?2
         WHERE id = ?1",
        params![local_id, now],
    )
    .map_err(db_error)?;
    Ok(())
}

pub fn delete_session(local_id: i64) -> Result<(), String> {
    let conn = history_connection().map_err(db_error)?;
    let deleted = conn
        .execute(
            "DELETE FROM agent_sessions WHERE id = ?1",
            params![local_id],
        )
        .map_err(db_error)?;
    if deleted == 0 {
        return Err(format!("Agent history session {local_id} was not found."));
    }
    log::info!("agent history deleted local_id={local_id}");
    Ok(())
}

pub fn map_codex_session(local_id: i64) -> Result<CodexMappingOutcome, String> {
    let Some(row) = lookup_session(local_id)? else {
        return Ok(CodexMappingOutcome::Skipped);
    };
    if row.provider_id != "codex" {
        update_mapping(
            row.id,
            row.cli_session_id.as_deref(),
            RestoreState::Unsupported,
            &row.metadata_json,
        )?;
        return Ok(CodexMappingOutcome::Unsupported);
    }
    if row.restore_state == RestoreState::Restorable && row.cli_session_id.is_some() {
        return Ok(CodexMappingOutcome::Restorable(
            row.cli_session_id.unwrap_or_default(),
        ));
    }

    match find_codex_thread(&row) {
        Ok(CodexThreadMatch::Matched(matched)) => {
            let metadata = json!({
                "codex_state_path": matched.state_path.to_string_lossy(),
                "matched_field": matched.field,
                "match_score": matched.score,
                "recency_at_ms": matched.recency_at_ms,
            })
            .to_string();
            update_mapping(
                row.id,
                Some(&matched.thread_id),
                RestoreState::Restorable,
                &metadata,
            )?;
            log::info!(
                "agent history mapped codex session local_id={} cli_session_id={} field={} score={}",
                row.id,
                matched.thread_id,
                matched.field,
                matched.score
            );
            Ok(CodexMappingOutcome::Restorable(matched.thread_id))
        }
        Ok(CodexThreadMatch::Ambiguous(metadata)) => {
            update_mapping(row.id, None, RestoreState::Ambiguous, &metadata)?;
            log::warn!("agent history codex mapping ambiguous local_id={}", row.id);
            Ok(CodexMappingOutcome::Ambiguous)
        }
        Ok(CodexThreadMatch::Missing(metadata)) => {
            update_mapping(row.id, None, RestoreState::Missing, &metadata)?;
            log::debug!("agent history codex mapping missing local_id={}", row.id);
            Ok(CodexMappingOutcome::Missing)
        }
        Err(err) => {
            let metadata = json!({ "error": err }).to_string();
            update_mapping(row.id, None, RestoreState::Missing, &metadata)?;
            Err(err)
        }
    }
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
            ON agent_session_tags(normalized_tag, session_id);",
    )?;
    ensure_history_columns(&conn)?;
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

fn lookup_by_title(
    conn: &Connection,
    workspace_key: &str,
    provider_id: &str,
    normalized_title: &str,
) -> Result<Option<AgentSessionRow>, String> {
    conn.query_row(
        "SELECT id, session_uuid, provider, workspace_key, git_remote_url, repo_path, title,
                normalized_title, task_description, normalized_task_description,
                cli_session_id, restore_state, metadata_json,
                created_at_ms, updated_at_ms, last_seen_at_ms, ended_at_ms
         FROM agent_sessions
         WHERE workspace_key = ?1 AND provider = ?2 AND normalized_title = ?3",
        params![workspace_key, provider_id, normalized_title],
        row_from_db,
    )
    .optional()
    .map_err(db_error)
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

enum CodexThreadMatch {
    Matched(MatchedCodexThread),
    Ambiguous(String),
    Missing(String),
}

struct MatchedCodexThread {
    thread_id: String,
    state_path: PathBuf,
    field: &'static str,
    score: i64,
    recency_at_ms: i64,
}

struct CodexThreadCandidate {
    id: String,
    title: String,
    preview: String,
    first_user_message: String,
    created_at_ms: i64,
    recency_at_ms: i64,
}

fn find_codex_thread(row: &AgentSessionRow) -> Result<CodexThreadMatch, String> {
    let Some(state_path) = newest_usable_codex_state_db() else {
        return Ok(CodexThreadMatch::Missing(
            json!({ "reason": "codex_state_db_not_found" }).to_string(),
        ));
    };

    let conn = Connection::open_with_flags(&state_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| format!("Failed to open Codex state {}: {err}", state_path.display()))?;
    conn.pragma_update(None, "busy_timeout", DB_BUSY_TIMEOUT_MS)
        .map_err(db_error)?;

    let cwd = row.repo_path.to_string_lossy().to_string();
    let mut stmt = conn
        .prepare(
            "SELECT id, title, preview, first_user_message, created_at_ms, recency_at_ms
             FROM threads
             WHERE source = 'cli'
               AND cwd = ?1
               AND archived = 0
               AND preview <> ''
             ORDER BY recency_at_ms DESC, updated_at_ms DESC",
        )
        .map_err(|err| {
            format!(
                "Failed to query Codex threads from {}: {err}",
                state_path.display()
            )
        })?;

    let candidates = stmt
        .query_map(params![cwd], |row| {
            Ok(CodexThreadCandidate {
                id: row.get(0)?,
                title: row.get(1)?,
                preview: row.get(2)?,
                first_user_message: row.get(3)?,
                created_at_ms: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                recency_at_ms: row.get(5)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;

    let Some(best) = best_codex_match(&row.title, &candidates)
        .or_else(|| best_codex_time_match(row, &candidates))
    else {
        return Ok(CodexThreadMatch::Missing(
            json!({
                "codex_state_path": state_path.to_string_lossy(),
                "candidate_count": candidates.len(),
            })
            .to_string(),
        ));
    };

    let best_score = best.score;
    let ambiguous = candidates.iter().filter_map(|candidate| {
        codex_candidate_match(&row.title, candidate).filter(|matched| matched.score == best_score)
    });
    let ambiguous_count = ambiguous.count();
    if ambiguous_count > 1 {
        return Ok(CodexThreadMatch::Ambiguous(
            json!({
                "codex_state_path": state_path.to_string_lossy(),
                "candidate_count": candidates.len(),
                "ambiguous_count": ambiguous_count,
                "candidate_cli_session_id": best.thread_id,
                "candidate_field": best.field,
                "candidate_recency_at_ms": best.recency_at_ms,
                "score": best_score,
            })
            .to_string(),
        ));
    }

    Ok(CodexThreadMatch::Matched(MatchedCodexThread {
        thread_id: best.thread_id,
        state_path,
        field: best.field,
        score: best.score,
        recency_at_ms: best.recency_at_ms,
    }))
}

fn newest_usable_codex_state_db() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let codex_dir = PathBuf::from(home).join(".codex");
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

struct CandidateMatch {
    thread_id: String,
    field: &'static str,
    score: i64,
    recency_at_ms: i64,
}

fn best_codex_match(title: &str, candidates: &[CodexThreadCandidate]) -> Option<CandidateMatch> {
    candidates
        .iter()
        .filter_map(|candidate| codex_candidate_match(title, candidate))
        .max_by_key(|matched| (matched.score, matched.recency_at_ms))
}

fn best_codex_time_match(
    row: &AgentSessionRow,
    candidates: &[CodexThreadCandidate],
) -> Option<CandidateMatch> {
    let mut timed = candidates
        .iter()
        .map(|candidate| {
            let distance_ms = (candidate.created_at_ms - row.created_at_ms).abs();
            (candidate, distance_ms)
        })
        .filter(|(_, distance_ms)| *distance_ms <= CODEX_TIME_MATCH_WINDOW_MS)
        .collect::<Vec<_>>();
    timed.sort_by_key(|(_, distance_ms)| *distance_ms);

    let (best, best_distance_ms) = timed.first()?;
    if timed.get(1).is_some_and(|(_, distance_ms)| {
        *distance_ms - *best_distance_ms < CODEX_TIME_MATCH_AMBIGUITY_MS
    }) {
        log::warn!(
            "agent history codex time match ambiguous local_id={} best_distance_ms={}",
            row.id,
            best_distance_ms
        );
        return None;
    }

    log::info!(
        "agent history codex time match selected local_id={} cli_session_id={} distance_ms={}",
        row.id,
        best.id,
        best_distance_ms
    );
    Some(CandidateMatch {
        thread_id: best.id.clone(),
        field: "created_at",
        score: 100,
        recency_at_ms: best.recency_at_ms,
    })
}

fn codex_candidate_match(title: &str, candidate: &CodexThreadCandidate) -> Option<CandidateMatch> {
    [
        ("title", candidate.title.as_str()),
        ("preview", candidate.preview.as_str()),
        ("first_user_message", candidate.first_user_message.as_str()),
    ]
    .into_iter()
    .filter_map(|(field, value)| match_score(title, value).map(|score| (field, score)))
    .max_by_key(|(_, score)| *score)
    .map(|(field, score)| CandidateMatch {
        thread_id: candidate.id.clone(),
        field,
        score,
        recency_at_ms: candidate.recency_at_ms,
    })
}

fn match_score(left: &str, right: &str) -> Option<i64> {
    let left = normalize_title_for_match(left);
    let right = normalize_title_for_match(right);
    if left.is_empty() || right.is_empty() {
        return None;
    }
    if left == right {
        return Some(300);
    }

    let left_chars = left.chars().count();
    let right_chars = right.chars().count();
    let length_ratio = left_chars.min(right_chars) as f64 / left_chars.max(right_chars) as f64;
    if length_ratio >= 0.85 && (left.starts_with(&right) || right.starts_with(&left)) {
        return Some(220);
    }
    if length_ratio >= 0.85 && (left.contains(&right) || right.contains(&left)) {
        return Some(180);
    }
    if left_chars >= 12 && right.contains(&left) {
        return Some(170);
    }
    if right_chars >= 12 && left.contains(&right) {
        return Some(165);
    }

    let left_tokens = token_set(&left);
    let right_tokens = token_set(&right);
    if left_tokens.len().min(right_tokens.len()) >= 4 {
        let overlap = left_tokens.intersection(&right_tokens).count();
        let overlap_ratio = overlap as f64 / left_tokens.len().max(right_tokens.len()) as f64;
        if overlap_ratio >= 0.90 {
            return Some(140);
        }
    }

    None
}

fn token_set(text: &str) -> HashSet<&str> {
    text.split_whitespace().collect()
}

fn metadata_string(metadata_json: &str, key: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(metadata_json).ok()?;
    value.get(key)?.as_str().map(ToString::to_string)
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
