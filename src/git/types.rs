use crate::github;
use std::time::SystemTime;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepositorySnapshot {
    pub name: String,
    pub branch: String,
    pub branches: Vec<BranchInfo>,
    pub remote_name: Option<String>,
    pub remote_url: Option<String>,
    pub remote_owner: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub has_upstream: bool,
    pub last_fetch_at: Option<SystemTime>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub github_avatar_url: Option<String>,
    pub warn_if_remote_owner_mismatch: bool,
    pub changed_files: Vec<ChangedFile>,
    pub history_head: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct GitSettings {
    pub global_user_name: Option<String>,
    pub global_user_email: Option<String>,
    pub local_user_name: Option<String>,
    pub local_user_email: Option<String>,
    pub use_global_user: bool,
    pub commit_timezone: Option<String>,
    pub warn_if_remote_owner_mismatch: bool,
    pub use_system_timezone: bool,
    pub github_auth_account: Option<github::GitHubAuthAccount>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
    pub git_status_bits: u32,
    pub worktree_signature: Option<ChangedFileSignature>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFileSignature {
    pub is_dir: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub comment: String,
    pub author: String,
    pub author_email: Option<String>,
    pub relative_time: String,
    pub insertions: usize,
    pub deletions: usize,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CommitMessage {
    pub summary: String,
    pub description: String,
}

#[derive(Clone, Debug, Default)]
pub struct CommitPage {
    pub commits: Vec<Commit>,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
    pub upstream: Option<String>,
    pub is_default: bool,
    pub is_recent: bool,
}

#[derive(Clone, Debug)]
pub struct FileComparison {
    pub rows: Vec<FileDiffRow>,
    pub fingerprint: u64,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct GitStatusEntry {
    pub(crate) status_code: String,
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
    pub(crate) unmerged: bool,
    pub(crate) untracked: bool,
}

pub(crate) fn parse_porcelain_status_entries(bytes: &[u8]) -> Vec<GitStatusEntry> {
    let tokens = bytes
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .map(|token| String::from_utf8_lossy(token).to_string())
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let field = &tokens[index];
        index += 1;

        match field.as_bytes().first().copied() {
            Some(b'1') => {
                if let Some(entry) = parse_porcelain_changed_entry(field) {
                    entries.push(entry);
                }
            }
            Some(b'2') => {
                let old_path = tokens.get(index).cloned();
                index += usize::from(old_path.is_some());
                if let Some(entry) = parse_porcelain_renamed_entry(field, old_path) {
                    entries.push(entry);
                }
            }
            Some(b'u') => {
                if let Some(entry) = parse_porcelain_unmerged_entry(field) {
                    entries.push(entry);
                }
            }
            Some(b'?') => {
                if let Some(path) = field.strip_prefix("? ").filter(|path| !path.is_empty()) {
                    entries.push(GitStatusEntry {
                        status_code: "??".to_string(),
                        path: path.to_string(),
                        old_path: None,
                        unmerged: false,
                        untracked: true,
                    });
                }
            }
            _ => {}
        }
    }

    entries
}

fn parse_porcelain_changed_entry(field: &str) -> Option<GitStatusEntry> {
    let mut parts = field.splitn(9, ' ');
    parts.next()?;
    let status_code = parts.next()?.to_string();
    for _ in 0..6 {
        parts.next()?;
    }
    let path = parts.next()?.to_string();
    Some(GitStatusEntry {
        status_code,
        path,
        old_path: None,
        unmerged: false,
        untracked: false,
    })
}

fn parse_porcelain_renamed_entry(field: &str, old_path: Option<String>) -> Option<GitStatusEntry> {
    let mut parts = field.splitn(10, ' ');
    parts.next()?;
    let status_code = parts.next()?.to_string();
    for _ in 0..6 {
        parts.next()?;
    }
    parts.next()?;
    let path = parts.next()?.to_string();
    Some(GitStatusEntry {
        status_code,
        path,
        old_path,
        unmerged: false,
        untracked: false,
    })
}

fn parse_porcelain_unmerged_entry(field: &str) -> Option<GitStatusEntry> {
    let mut parts = field.splitn(11, ' ');
    parts.next()?;
    let status_code = parts.next()?.to_string();
    for _ in 0..8 {
        parts.next()?;
    }
    let path = parts.next()?.to_string();
    Some(GitStatusEntry {
        status_code,
        path,
        old_path: None,
        unmerged: true,
        untracked: false,
    })
}

pub(crate) fn status_entry_visible(entry: &GitStatusEntry) -> bool {
    entry.status_code != "AD"
}

pub(crate) fn changed_file_from_porcelain_entry(entry: &GitStatusEntry) -> ChangedFile {
    ChangedFile {
        status: porcelain_entry_status_label(entry).to_string(),
        path: entry.path.clone(),
        git_status_bits: 0,
        worktree_signature: None,
    }
}

fn porcelain_entry_status_label(entry: &GitStatusEntry) -> &'static str {
    if entry.unmerged || entry.status_code.contains('U') {
        "U"
    } else if entry.status_code.contains('R') || entry.status_code.contains('C') {
        "R"
    } else if entry.status_code.contains('D') {
        "D"
    } else if entry.untracked || entry.status_code.contains('A') || entry.status_code.contains('?')
    {
        "A"
    } else {
        "M"
    }
}

pub(crate) fn porcelain_entry_matches_path(entry: &GitStatusEntry, path: &str) -> bool {
    entry.path == path || entry.old_path.as_deref() == Some(path)
}

pub(crate) fn porcelain_entry_force_remove_paths(entry: &GitStatusEntry) -> Vec<String> {
    if entry.old_path.is_some() || entry.status_code.contains('D') {
        return vec![entry.old_path.as_ref().unwrap_or(&entry.path).clone()];
    }
    Vec::new()
}

pub(crate) fn porcelain_entry_update_paths(entry: &GitStatusEntry) -> Vec<String> {
    if entry.status_code.contains('D') && entry.old_path.is_none() {
        return Vec::new();
    }
    vec![entry.path.clone()]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Mixed,
    Hard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoMetadata {
    Fork,
    Private,
    Public,
    Local,
    Unknown,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRepositoryMetadata {
    pub kind: RepoMetadata,
    pub remote_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BytesComparison {
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
    pub fingerprint: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Deleted,
    Added,
    Fold,
}

#[derive(Clone, Debug)]
pub struct FileDiffRow {
    pub left_number: Option<usize>,
    pub right_number: Option<usize>,
    pub left_text: Option<String>,
    pub right_text: Option<String>,
    pub left_kind: DiffKind,
    pub right_kind: DiffKind,
}

impl FileComparison {
    pub(crate) fn from_rows(rows: Vec<FileDiffRow>) -> Self {
        let fingerprint = fingerprint_diff_rows(&rows);
        let insertions = rows
            .iter()
            .filter(|row| row.right_kind == DiffKind::Added)
            .count();
        let deletions = rows
            .iter()
            .filter(|row| row.left_kind == DiffKind::Deleted)
            .count();
        Self {
            rows,
            fingerprint,
            insertions,
            deletions,
        }
    }
}

impl BytesComparison {
    pub(crate) fn from_parts(before: Option<Vec<u8>>, after: Option<Vec<u8>>) -> Self {
        let fingerprint = fingerprint_optional_bytes(before.as_deref(), after.as_deref());
        Self {
            before,
            after,
            fingerprint,
        }
    }
}

fn fingerprint_diff_rows(rows: &[FileDiffRow]) -> u64 {
    let mut fingerprint = 0xcbf29ce484222325;
    hash_usize(&mut fingerprint, rows.len());
    for row in rows {
        hash_option_usize(&mut fingerprint, row.left_number);
        hash_option_usize(&mut fingerprint, row.right_number);
        hash_kind(&mut fingerprint, row.left_kind);
        hash_kind(&mut fingerprint, row.right_kind);
        hash_option_text(&mut fingerprint, row.left_text.as_deref());
        hash_option_text(&mut fingerprint, row.right_text.as_deref());
    }
    fingerprint
}

fn fingerprint_optional_bytes(before: Option<&[u8]>, after: Option<&[u8]>) -> u64 {
    let mut fingerprint = 0xcbf29ce484222325;
    hash_optional_bytes(&mut fingerprint, before);
    hash_optional_bytes(&mut fingerprint, after);
    fingerprint
}

fn hash_option_usize(fingerprint: &mut u64, value: Option<usize>) {
    match value {
        Some(value) => {
            hash_u8(fingerprint, 1);
            hash_usize(fingerprint, value);
        }
        None => hash_u8(fingerprint, 0),
    }
}

fn hash_kind(fingerprint: &mut u64, kind: DiffKind) {
    hash_u8(
        fingerprint,
        match kind {
            DiffKind::Context => 0,
            DiffKind::Deleted => 1,
            DiffKind::Added => 2,
            DiffKind::Fold => 3,
        },
    );
}

fn hash_option_text(fingerprint: &mut u64, text: Option<&str>) {
    match text {
        Some(text) => {
            hash_u8(fingerprint, 1);
            hash_usize(fingerprint, text.len());
            hash_bytes(fingerprint, text.as_bytes());
        }
        None => hash_u8(fingerprint, 0),
    }
}

fn hash_optional_bytes(fingerprint: &mut u64, bytes: Option<&[u8]>) {
    match bytes {
        Some(bytes) => {
            hash_u8(fingerprint, 1);
            hash_usize(fingerprint, bytes.len());
            hash_bytes(fingerprint, bytes);
        }
        None => hash_u8(fingerprint, 0),
    }
}

fn hash_usize(fingerprint: &mut u64, value: usize) {
    hash_bytes(fingerprint, &value.to_le_bytes());
}

fn hash_u8(fingerprint: &mut u64, value: u8) {
    hash_bytes(fingerprint, &[value]);
}

fn hash_bytes(fingerprint: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *fingerprint ^= u64::from(*byte);
        *fingerprint = fingerprint.wrapping_mul(0x100000001b3);
    }
}
