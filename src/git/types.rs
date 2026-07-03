use super::*;

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

pub const RECENT_BRANCHES_LIMIT: usize = 5;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchKind {
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
    pub kind: BranchKind,
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

#[derive(Clone, Debug, Default)]
pub(crate) struct FilePathPair {
    pub(crate) old_path: Option<String>,
    pub(crate) new_path: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct GitStatusEntry {
    pub(crate) status_code: String,
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
    pub(crate) unmerged: bool,
    pub(crate) untracked: bool,
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
