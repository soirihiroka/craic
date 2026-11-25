use super::NativeSession;
use crate::agent_history::{DB_BUSY_TIMEOUT_MS, db_error};
use reqwest::Url;
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(super) fn list(cwd: &Path) -> Result<Option<Vec<NativeSession>>, String> {
    let Some(data_dir) =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".gemini/antigravity-cli"))
    else {
        return Ok(None);
    };
    let index = data_dir.join("conversation_summaries.db");
    if !index.try_exists().map_err(|error| error.to_string())? {
        return Ok(None);
    }
    let conn =
        Connection::open_with_flags(index, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db_error)?;
    conn.pragma_update(None, "busy_timeout", DB_BUSY_TIMEOUT_MS)
        .map_err(db_error)?;
    let mut stmt = conn
        .prepare(
            "SELECT conversation_id, workspace_uris,
                    COALESCE(NULLIF(TRIM(title), ''), substr(preview, 1, 240)),
                    COALESCE(CAST(unixepoch(last_modified_time, 'subsec') * 1000 AS INTEGER), 0)
             FROM conversation_summaries
             WHERE workspace_uris <> '' AND parent_conversation_id = '' AND step_count > 0
               AND (title <> '' OR preview <> '')",
        )
        .map_err(db_error)?;
    let mut rows = stmt.query([]).map_err(db_error)?;
    let conversations = data_dir.join("conversations");
    let canonical_cwd = fs::canonicalize(cwd).ok();
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().map_err(db_error)? {
        let workspace_uris: String = row.get(1).map_err(db_error)?;
        let Ok(workspace_uris) = serde_json::from_str::<Vec<String>>(&workspace_uris) else {
            continue;
        };
        let in_workspace = workspace_uris
            .iter()
            .filter_map(|uri| Url::parse(uri).ok()?.to_file_path().ok())
            .any(|path| {
                path == cwd
                    || canonical_cwd.as_ref().is_some_and(|canonical_cwd| {
                        &path == canonical_cwd
                            || fs::canonicalize(path).is_ok_and(|path| &path == canonical_cwd)
                    })
            });
        if !in_workspace {
            continue;
        }

        // The index can retain rows after AGY has removed the conversation.
        let id: String = row.get(0).map_err(db_error)?;
        if Uuid::parse_str(&id).is_err()
            || !(conversations.join(format!("{id}.db")).is_file()
                || conversations.join(format!("{id}.pb")).is_file())
        {
            continue;
        }
        let updated_at_ms = row.get(3).map_err(db_error)?;
        sessions.push(NativeSession {
            provider: "agy",
            id,
            title: row.get(2).map_err(db_error)?,
            created_at_ms: updated_at_ms,
            updated_at_ms,
        });
    }
    Ok(Some(sessions))
}
