use super::*;

pub fn commit_details(path: &Path, hash: &str) -> Result<Commit, String> {
    let output = run_git(
        path,
        &[
            "show",
            "-s",
            "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%ct%x1f%B",
            hash,
        ],
    )?;
    let tags = tags_for_commit(path, hash).unwrap_or_default();
    let (insertions, deletions) = commit_line_stats(path, hash).unwrap_or_default();
    parse_commit_details(&output, tags, insertions, deletions)
}

pub fn commit_message(path: &Path, hash: &str) -> Result<CommitMessage, String> {
    let message = run_git(path, &["show", "-s", "--format=%B", hash])?;
    let (summary, description) = commit_message_parts(&message);

    Ok(CommitMessage {
        summary,
        description,
    })
}

pub(crate) fn commit_message_parts(message: &str) -> (String, String) {
    let message = message.trim_end();
    let mut parts = message.splitn(2, '\n');
    let summary = parts.next().unwrap_or_default().trim().to_string();
    let description = parts
        .next()
        .unwrap_or_default()
        .trim_start_matches('\n')
        .trim_end()
        .to_string();

    (summary, description)
}

pub fn commit_parent_hash(path: &Path, hash: &str) -> Result<Option<String>, String> {
    let output = run_git(path, &["rev-list", "--parents", "-n", "1", hash])?;
    Ok(output.split_whitespace().nth(1).map(ToString::to_string))
}

pub fn tags_for_commit(path: &Path, hash: &str) -> Result<Vec<String>, String> {
    let mut tags = run_git(path, &["tag", "--points-at", hash])?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    tags.sort();
    Ok(tags)
}

pub fn commit_page(path: &Path, after: Option<&str>, limit: usize) -> Result<CommitPage, String> {
    paged_commits(path, after, limit)
}

pub fn commit_search_page(
    path: &Path,
    query: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<CommitPage, String> {
    if query.trim().is_empty() {
        return paged_commits(path, after, limit);
    }
    paged_commit_search(path, query, after, limit)
}

pub fn commit_changed_files(path: &Path, hash: &str) -> Result<Vec<ChangedFile>, String> {
    let output = run_git_bytes(
        path,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-M",
            "-z",
            hash,
        ],
    )?;
    let mut files = parse_name_status_files_z(&output);

    sort_changed_files(&mut files);
    Ok(files)
}

pub(crate) fn history_head(root: &Path) -> Option<String> {
    run_git(root, &["rev-parse", "HEAD"]).ok()
}

pub(crate) fn paged_commits(
    path: &Path,
    after: Option<&str>,
    limit: usize,
) -> Result<CommitPage, String> {
    let hashes = rev_list_hashes(path, after, limit)?;
    commits_from_hashes(path, hashes, limit, |_| true)
}

pub(crate) fn paged_commit_search(
    path: &Path,
    query: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<CommitPage, String> {
    let needle = query.to_lowercase();
    let hashes = rev_list_hashes(path, after, usize::MAX)?;
    commits_from_hashes(path, hashes, limit, |commit| {
        commit_search_text(commit).to_lowercase().contains(&needle)
    })
}

pub(crate) fn rev_list_hashes(
    path: &Path,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<String>, String> {
    let output = run_git(path, &["rev-list", "HEAD"])?;
    let mut hashes = Vec::new();
    let mut collecting = after.is_none();
    let fetch_limit = limit.saturating_add(1);

    for hash in output
        .lines()
        .map(str::trim)
        .filter(|hash| !hash.is_empty())
    {
        if !collecting {
            if Some(hash) == after {
                collecting = true;
            }
            continue;
        }

        hashes.push(hash.to_string());
        if hashes.len() >= fetch_limit {
            break;
        }
    }

    Ok(hashes)
}

pub(crate) fn commits_from_hashes(
    path: &Path,
    hashes: Vec<String>,
    limit: usize,
    mut include: impl FnMut(&Commit) -> bool,
) -> Result<CommitPage, String> {
    let mut commits = Vec::new();
    let mut has_more = false;

    for hash in hashes {
        let commit = commit_details(path, &hash)?;
        if !include(&commit) {
            continue;
        }
        if commits.len() == limit {
            has_more = true;
            break;
        }
        commits.push(commit);
    }

    Ok(CommitPage { commits, has_more })
}

pub(crate) fn commit_search_text(commit: &Commit) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        commit.hash,
        commit.short_hash,
        commit.subject,
        commit.comment,
        commit.author,
        commit.author_email.as_deref().unwrap_or_default()
    )
}

pub(crate) fn parse_commit_details(
    output: &str,
    tags: Vec<String>,
    insertions: usize,
    deletions: usize,
) -> Result<Commit, String> {
    let mut parts = output.splitn(6, '\x1f');
    let hash = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Commit details did not include a hash.".to_string())?
        .trim()
        .to_string();
    let short_hash = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| short_hash(&hash).to_string());
    let author = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "Unknown author".to_string());
    let author_email = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let timestamp = parts
        .next()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let message = parts.next().unwrap_or_default();
    let (subject, comment) = commit_message_parts(message);

    Ok(Commit {
        hash,
        short_hash,
        subject: if subject.is_empty() {
            "Untitled commit".to_string()
        } else {
            subject
        },
        comment,
        author,
        author_email,
        relative_time: relative_time(timestamp),
        insertions,
        deletions,
        tags,
    })
}

pub(crate) fn commit_line_stats(path: &Path, hash: &str) -> Result<(usize, usize), String> {
    let output = run_git(path, &["show", "--numstat", "--format=", hash])?;
    Ok(numstat_totals(&output))
}

pub(crate) fn numstat_totals(output: &str) -> (usize, usize) {
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    for line in output.lines() {
        let mut fields = line.split('\t');
        let Some(added) = fields.next() else {
            continue;
        };
        let Some(deleted) = fields.next() else {
            continue;
        };
        insertions += added.parse::<usize>().unwrap_or(0);
        deletions += deleted.parse::<usize>().unwrap_or(0);
    }
    (insertions, deletions)
}

pub(crate) fn parse_name_status_files_z(bytes: &[u8]) -> Vec<ChangedFile> {
    parse_name_status_entries_z(bytes)
        .into_iter()
        .map(|entry| ChangedFile {
            status: name_status_label(&entry.status).to_string(),
            path: entry.new_path.or(entry.old_path).unwrap_or_default(),
            git_status_bits: 0,
            worktree_signature: None,
        })
        .filter(|file| !file.path.is_empty())
        .collect()
}

pub(crate) fn parse_name_status_path_pairs_z(bytes: &[u8]) -> Vec<FilePathPair> {
    parse_name_status_entries_z(bytes)
        .into_iter()
        .map(|entry| FilePathPair {
            old_path: entry.old_path,
            new_path: entry.new_path,
        })
        .collect()
}

struct NameStatusEntry {
    status: String,
    old_path: Option<String>,
    new_path: Option<String>,
}

fn parse_name_status_entries_z(bytes: &[u8]) -> Vec<NameStatusEntry> {
    let tokens = bytes
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .map(|token| String::from_utf8_lossy(token).to_string())
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let status = tokens[index].clone();
        index += 1;
        let Some(kind) = status.chars().next() else {
            continue;
        };

        match kind {
            'R' | 'C' => {
                let old_path = tokens.get(index).cloned();
                let new_path = tokens.get(index + 1).cloned();
                index += usize::from(old_path.is_some()) + usize::from(new_path.is_some());
                entries.push(NameStatusEntry {
                    status,
                    old_path,
                    new_path,
                });
            }
            'A' => {
                let new_path = tokens.get(index).cloned();
                index += usize::from(new_path.is_some());
                entries.push(NameStatusEntry {
                    status,
                    old_path: None,
                    new_path,
                });
            }
            'D' => {
                let old_path = tokens.get(index).cloned();
                index += usize::from(old_path.is_some());
                entries.push(NameStatusEntry {
                    status,
                    old_path,
                    new_path: None,
                });
            }
            _ => {
                let path = tokens.get(index).cloned();
                index += usize::from(path.is_some());
                entries.push(NameStatusEntry {
                    status,
                    old_path: path.clone(),
                    new_path: path,
                });
            }
        }
    }

    entries
}

pub(crate) fn name_status_label(status: &str) -> &'static str {
    match status.chars().next() {
        Some('A') => "A",
        Some('D') => "D",
        Some('R') | Some('C') => "R",
        Some('U') => "U",
        _ => "M",
    }
}

pub(crate) fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

pub(crate) fn relative_time(seconds: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(seconds);
    let elapsed = now.saturating_sub(seconds);

    match elapsed {
        0..=59 => "just now".to_string(),
        60..=3_599 => plural(elapsed / 60, "minute"),
        3_600..=86_399 => plural(elapsed / 3_600, "hour"),
        86_400..=2_592_000 => plural(elapsed / 86_400, "day"),
        _ => plural(elapsed / 2_592_000, "month"),
    }
}

pub(crate) fn plural(value: i64, unit: &str) -> String {
    if value == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{value} {unit}s ago")
    }
}

pub(crate) const MAX_BINARY_PREVIEW_BYTES: usize = 32 * 1024 * 1024;
