mod codex;
pub use codex::CodexHistoryReader;
mod grok;
pub use grok::GrokHistoryReader;

use super::*;
use std::collections::HashMap;

struct NativeSession {
    provider: &'static str,
    id: String,
    title: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}

pub struct NativeHistory {
    pub rows: Vec<AgentSessionRow>,
    pub tags: Vec<WorkspaceTag>,
    pub warnings: Vec<String>,
}

/// Provider history is authoritative. Local rows only attach summaries and tags
/// to an exact provider/session ID; terminal titles are never used as identity.
pub fn list_native_sessions(
    workspace: &WorkspaceKey,
    limit: usize,
    title_query: Option<&str>,
    tag_filters: &[String],
    codex: Result<&CodexHistoryReader, String>,
    grok: Result<&GrokHistoryReader, String>,
) -> Result<NativeHistory, String> {
    let mut sessions = Vec::new();
    let mut warnings = Vec::new();
    let provider_limit =
        if title_query.is_some_and(|query| !query.trim().is_empty()) || !tag_filters.is_empty() {
            usize::MAX
        } else {
            limit
        };
    let results = std::thread::scope(|scope| {
        let codex = scope
            .spawn(|| codex.and_then(|reader| reader.list(workspace.repo_path(), provider_limit)));
        let grok = scope
            .spawn(|| grok.and_then(|reader| reader.list(workspace.repo_path(), provider_limit)));
        let mut opencode = Vec::new();
        let opencode =
            read_opencode_sessions(workspace.repo_path(), &mut opencode).map(|()| opencode);
        [
            (
                "Codex",
                codex
                    .join()
                    .unwrap_or_else(|_| Err("Codex history worker failed".to_string())),
            ),
            (
                "Grok",
                grok.join()
                    .unwrap_or_else(|_| Err("Grok history worker failed".to_string())),
            ),
            ("OpenCode", opencode),
        ]
    });
    for (provider, result) in results {
        match result {
            Ok(rows) => sessions.extend(rows),
            Err(error) => warnings.push(format!("{provider} history: {error}")),
        }
    }

    let mut conn = history_connection().map_err(db_error)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let mut native_ids = HashSet::new();
    for mut session in sessions {
        session.title = normalize_title(&session.title);
        if session.title.is_empty() {
            session.title = format!("Untitled {} thread", session.provider);
        }
        let identity = format!("native:{}:{}", session.provider, session.id);
        let existing = tx
            .query_row(
                "SELECT id FROM agent_sessions WHERE provider = ?1 AND workspace_key = ?2
             AND cli_session_id = ?3 ORDER BY (normalized_title = ?4) DESC, id DESC LIMIT 1",
                params![session.provider, workspace.key(), session.id, identity],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(id) = existing {
            tx.execute(
                "UPDATE agent_sessions SET title = ?2, normalized_title = ?3,
                 created_at_ms = ?4, updated_at_ms = ?5, last_seen_at_ms = ?5,
                 restore_state = 'restorable', session_uuid = COALESCE(session_uuid, ?6)
                 WHERE id = ?1 AND (title <> ?2 OR normalized_title <> ?3
                   OR created_at_ms <> ?4 OR updated_at_ms <> ?5 OR last_seen_at_ms <> ?5
                   OR restore_state <> 'restorable' OR session_uuid IS NULL)",
                params![
                    id,
                    session.title,
                    identity,
                    session.created_at_ms,
                    session.updated_at_ms,
                    new_session_uuid()
                ],
            )
            .map_err(db_error)?;
            native_ids.insert(id);
        } else {
            tx.execute(
                "INSERT INTO agent_sessions
                 (session_uuid, provider, workspace_key, repo_path, title, normalized_title,
                  cli_session_id, restore_state, metadata_json, created_at_ms, updated_at_ms, last_seen_at_ms)
                 VALUES (?9, ?2, ?3, ?4, ?5, ?6, ?1, 'restorable', '{}', ?7, ?8, ?8)",
                params![session.id, session.provider, workspace.key(), workspace.repo_path().to_string_lossy(),
                    session.title, identity, session.created_at_ms, session.updated_at_ms, new_session_uuid()],
            ).map_err(db_error)?;
            native_ids.insert(tx.last_insert_rowid());
        }
    }
    tx.commit().map_err(db_error)?;

    let mut rows = list_sessions(workspace.key(), i64::MAX as usize, 0, None, &[])?;
    rows.retain(|row| {
        native_ids.contains(&row.id)
            && !serde_json::from_str::<Value>(&row.metadata_json)
                .ok()
                .and_then(|metadata| metadata.get("hidden").and_then(Value::as_bool))
                .unwrap_or(false)
    });
    native_ids = rows.iter().map(|row| row.id).collect();
    let mut tag_counts = HashMap::<String, usize>::new();
    let mut tags_by_id = HashMap::<i64, Vec<String>>::new();
    let mut stmt = conn
        .prepare(
            "SELECT tag.session_id, tag.tag FROM agent_session_tags tag
         JOIN agent_sessions session ON session.id = tag.session_id
         WHERE session.workspace_key = ?1",
        )
        .map_err(db_error)?;
    for tag in stmt
        .query_map(params![workspace.key()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?
    {
        let (id, tag) = tag.map_err(db_error)?;
        if native_ids.contains(&id) {
            *tag_counts.entry(tag.clone()).or_default() += 1;
            tags_by_id.entry(id).or_default().push(tag);
        }
    }
    let query = title_query
        .map(normalize_title_for_match)
        .unwrap_or_default();
    rows.retain(|row| {
        (query.is_empty()
            || normalize_title_for_match(&row.title).contains(&query)
            || row
                .normalized_task_description
                .as_deref()
                .is_some_and(|text| text.contains(&query)))
            && (tag_filters.is_empty()
                || tags_by_id.get(&row.id).is_some_and(|tags| {
                    tag_filters
                        .iter()
                        .any(|filter| tags.contains(&normalize_tag(filter)))
                }))
    });
    rows.truncate(limit);
    let mut tags = tag_counts
        .into_iter()
        .map(|(tag, session_count)| WorkspaceTag { tag, session_count })
        .collect::<Vec<_>>();
    tags.sort_by(|a, b| {
        b.session_count
            .cmp(&a.session_count)
            .then_with(|| a.tag.cmp(&b.tag))
    });
    Ok(NativeHistory {
        rows,
        tags,
        warnings,
    })
}

fn read_opencode_sessions(cwd: &Path, sessions: &mut Vec<NativeSession>) -> Result<(), String> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let Some(path) = data
        .map(|data| data.join("opencode/opencode.db"))
        .filter(|path| path.is_file())
    else {
        return Ok(());
    };
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db_error)?;
    conn.pragma_update(None, "busy_timeout", DB_BUSY_TIMEOUT_MS)
        .map_err(db_error)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, title, time_created, time_updated FROM session
         WHERE directory = ?1 AND time_archived IS NULL AND parent_id IS NULL",
        )
        .map_err(db_error)?;
    sessions.extend(
        stmt.query_map(params![cwd.to_string_lossy()], |row| {
            Ok(NativeSession {
                provider: "opencode",
                id: row.get(0)?,
                title: row.get(1)?,
                created_at_ms: row.get(2)?,
                updated_at_ms: row.get(3)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?,
    );
    Ok(())
}
