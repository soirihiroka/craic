use super::*;

pub fn comparison(path: &Path, file_path: &str) -> Result<FileComparison, String> {
    let start = Instant::now();
    let paths = worktree_file_path_pair(path, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);
    ensure_worktree_text_previewable(path, old_path, new_path)?;

    let diff = worktree_diff(path, &paths, file_path)?;
    let rows = complete_diff_rows(
        parse_unified_diff(&diff),
        &head_file_lines(path, old_path)?,
        &workdir_file_lines(path, new_path)?,
        paths_changed(&paths),
    );
    log::info!(
        "git worktree comparison complete path={} rows={} elapsed_ms={}",
        file_path,
        rows.len(),
        start.elapsed().as_millis()
    );

    Ok(FileComparison::from_rows(rows))
}

pub fn commit_comparison(
    path: &Path,
    hash: &str,
    file_path: &str,
) -> Result<FileComparison, String> {
    let start = Instant::now();
    let paths = commit_file_path_pair(path, hash, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);
    ensure_commit_text_previewable(path, hash, old_path, new_path)?;
    let parent = commit_parent_hash(path, hash)?;
    let diff = commit_diff(path, hash, &paths, file_path)?;
    let rows = complete_diff_rows(
        parse_unified_diff(&diff),
        &tree_file_lines_opt(path, parent.as_deref(), old_path)?,
        &tree_file_lines(path, hash, new_path)?,
        paths_changed(&paths),
    );
    log::info!(
        "git commit comparison complete hash={} path={} rows={} elapsed_ms={}",
        short_hash(hash),
        file_path,
        rows.len(),
        start.elapsed().as_millis()
    );

    Ok(FileComparison::from_rows(rows))
}

pub fn bytes_comparison(path: &Path, file_path: &str) -> Result<BytesComparison, String> {
    let paths = worktree_file_path_pair(path, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);

    Ok(BytesComparison::from_parts(
        tree_file_binary_bytes_opt(path, Some("HEAD"), old_path)?,
        workdir_binary_bytes(path, new_path)?,
    ))
}

pub fn commit_bytes_comparison(
    path: &Path,
    hash: &str,
    file_path: &str,
) -> Result<BytesComparison, String> {
    let paths = commit_file_path_pair(path, hash, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);
    let parent = commit_parent_hash(path, hash)?;

    Ok(BytesComparison::from_parts(
        tree_file_binary_bytes_opt(path, parent.as_deref(), old_path)?,
        tree_file_binary_bytes_opt(path, Some(hash), new_path)?,
    ))
}

pub(crate) fn ensure_worktree_text_previewable(
    repo_path: &Path,
    old_path: &str,
    new_path: &str,
) -> Result<(), String> {
    ensure_tree_text_previewable(repo_path, Some("HEAD"), old_path)?;
    ensure_workdir_text_previewable(repo_path, new_path)
}

pub(crate) fn ensure_commit_text_previewable(
    repo_path: &Path,
    hash: &str,
    old_path: &str,
    new_path: &str,
) -> Result<(), String> {
    let parent = commit_parent_hash(repo_path, hash)?;
    ensure_tree_text_previewable(repo_path, parent.as_deref(), old_path)?;
    ensure_tree_text_previewable(repo_path, Some(hash), new_path)
}

pub(crate) fn complete_diff_rows(
    rows: Vec<FileDiffRow>,
    left_lines: &[String],
    right_lines: &[String],
    complete_empty: bool,
) -> Vec<FileDiffRow> {
    if rows.is_empty() {
        if complete_empty {
            let mut complete = Vec::new();
            append_context_gap(
                &mut complete,
                left_lines,
                right_lines,
                1,
                left_lines.len().saturating_add(1),
                1,
                right_lines.len().saturating_add(1),
            );
            return complete;
        }
        return rows;
    }

    let mut complete = Vec::new();
    let mut next_left = 1;
    let mut next_right = 1;

    for row in rows {
        if let (Some(left_number), Some(right_number)) = (row.left_number, row.right_number) {
            append_context_gap(
                &mut complete,
                left_lines,
                right_lines,
                next_left,
                left_number,
                next_right,
                right_number,
            );
        }

        if let Some(number) = row.left_number {
            next_left = number.saturating_add(1);
        }
        if let Some(number) = row.right_number {
            next_right = number.saturating_add(1);
        }

        complete.push(row);
    }

    append_context_gap(
        &mut complete,
        left_lines,
        right_lines,
        next_left,
        left_lines.len().saturating_add(1),
        next_right,
        right_lines.len().saturating_add(1),
    );

    complete
}

pub(crate) fn append_context_gap(
    rows: &mut Vec<FileDiffRow>,
    left_lines: &[String],
    right_lines: &[String],
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) {
    let count = left_end
        .saturating_sub(left_start)
        .min(right_end.saturating_sub(right_start));

    for offset in 0..count {
        let left_number = left_start + offset;
        let right_number = right_start + offset;
        let text = left_lines
            .get(left_number.saturating_sub(1))
            .or_else(|| right_lines.get(right_number.saturating_sub(1)))
            .cloned()
            .unwrap_or_default();

        rows.push(FileDiffRow {
            left_number: Some(left_number),
            right_number: Some(right_number),
            left_text: Some(text.clone()),
            right_text: Some(text),
            left_kind: DiffKind::Context,
            right_kind: DiffKind::Context,
        });
    }
}

pub(crate) fn is_file_path_match(file_path: &str, paths: &FilePathPair) -> bool {
    paths
        .old_path
        .as_deref()
        .is_some_and(|old_path| old_path == file_path)
        || paths
            .new_path
            .as_deref()
            .is_some_and(|new_path| new_path == file_path)
}

pub(crate) fn paths_changed(paths: &FilePathPair) -> bool {
    paths.old_path.is_some() && paths.new_path.is_some() && paths.old_path != paths.new_path
}

pub(crate) fn worktree_file_path_pair(
    path: &Path,
    file_path: &str,
) -> Result<FilePathPair, String> {
    let entries = status_entries(path)?;
    Ok(worktree_file_path_pair_from_entries(&entries, file_path))
}

pub(crate) fn worktree_file_path_pair_from_entries(
    entries: &[GitStatusEntry],
    file_path: &str,
) -> FilePathPair {
    entries
        .iter()
        .find(|entry| porcelain_entry_matches_path(entry, file_path))
        .map(|entry| FilePathPair {
            old_path: if entry.untracked {
                None
            } else {
                entry.old_path.clone().or_else(|| Some(entry.path.clone()))
            },
            new_path: Some(entry.path.clone()),
        })
        .unwrap_or_else(|| FilePathPair {
            old_path: Some(file_path.to_string()),
            new_path: Some(file_path.to_string()),
        })
}

pub(crate) fn commit_file_path_pair(
    path: &Path,
    hash: &str,
    file_path: &str,
) -> Result<FilePathPair, String> {
    Ok(commit_name_status_entries(path, hash)?
        .into_iter()
        .find(|paths| is_file_path_match(file_path, paths))
        .unwrap_or_else(|| FilePathPair {
            old_path: Some(file_path.to_string()),
            new_path: Some(file_path.to_string()),
        }))
}

pub(crate) fn commit_name_status_entries(
    path: &Path,
    hash: &str,
) -> Result<Vec<FilePathPair>, String> {
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
    Ok(parse_name_status_path_pairs_z(&output))
}

pub(crate) fn commit_file_path_pair_from_name_status_bytes(
    bytes: &[u8],
    file_path: &str,
) -> FilePathPair {
    parse_name_status_path_pairs_z(bytes)
        .into_iter()
        .find(|paths| is_file_path_match(file_path, paths))
        .unwrap_or_else(|| FilePathPair {
            old_path: Some(file_path.to_string()),
            new_path: Some(file_path.to_string()),
        })
}

pub(crate) fn worktree_diff(
    path: &Path,
    paths: &FilePathPair,
    fallback_path: &str,
) -> Result<String, String> {
    if paths.old_path.is_none()
        && let Some(new_path) = paths.new_path.as_deref()
    {
        let root = repo_root(path)?;
        return run_git_owned_with_success_codes(
            path,
            &[
                "diff".to_string(),
                "--no-index".to_string(),
                "--no-ext-diff".to_string(),
                "--no-color".to_string(),
                "--unified=3".to_string(),
                "--".to_string(),
                "/dev/null".to_string(),
                root.join(new_path).display().to_string(),
            ],
            &[0, 1],
        );
    }

    let mut args = vec![
        "diff".to_string(),
        "HEAD".to_string(),
        "--no-ext-diff".to_string(),
        "--find-renames".to_string(),
        "--no-color".to_string(),
        "--unified=3".to_string(),
    ];
    args.extend(diff_args_for_paths(&[
        paths.old_path.as_deref(),
        paths.new_path.as_deref(),
        Some(fallback_path),
    ]));
    run_git_owned(path, &args)
}

pub(crate) fn commit_diff(
    path: &Path,
    hash: &str,
    paths: &FilePathPair,
    fallback_path: &str,
) -> Result<String, String> {
    let mut args = vec![
        "show".to_string(),
        "--format=".to_string(),
        "--find-renames".to_string(),
        "--no-ext-diff".to_string(),
        "--no-color".to_string(),
        "--unified=3".to_string(),
        hash.to_string(),
    ];
    args.extend(diff_args_for_paths(&[
        paths.old_path.as_deref(),
        paths.new_path.as_deref(),
        Some(fallback_path),
    ]));
    run_git_owned(path, &args)
}

pub(crate) fn diff_args_for_paths(paths: &[Option<&str>]) -> Vec<String> {
    let mut args = vec!["--".to_string()];
    let mut seen = HashSet::new();
    for path in paths.iter().flatten().filter(|path| !path.is_empty()) {
        if seen.insert((*path).to_string()) {
            args.push((*path).to_string());
        }
    }
    args
}

pub(crate) fn head_file_lines(repo_path: &Path, file_path: &str) -> Result<Vec<String>, String> {
    tree_file_lines_opt(repo_path, Some("HEAD"), file_path)
}

pub(crate) fn workdir_file_lines(repo_path: &Path, file_path: &str) -> Result<Vec<String>, String> {
    let bytes = match workdir_text_bytes(repo_path, file_path)? {
        Some(bytes) => bytes,
        None => return Ok(Vec::new()),
    };
    Ok(lines_from_bytes(&bytes))
}

pub(crate) fn tree_file_lines_opt(
    repo_path: &Path,
    rev: Option<&str>,
    file_path: &str,
) -> Result<Vec<String>, String> {
    let Some(rev) = rev else {
        return Ok(Vec::new());
    };
    tree_file_lines(repo_path, rev, file_path)
}

pub(crate) fn tree_file_lines(
    repo_path: &Path,
    rev: &str,
    file_path: &str,
) -> Result<Vec<String>, String> {
    let Some(bytes) = tree_file_bytes(repo_path, rev, file_path, MAX_TEXT_PREVIEW_BYTES)? else {
        return Ok(Vec::new());
    };
    ensure_blob_text_previewable(&bytes)?;
    Ok(lines_from_bytes(&bytes))
}

pub(crate) fn comparison_from_unified_diff(
    diff: &str,
    left_lines: &[String],
    right_lines: &[String],
    complete_empty: bool,
) -> FileComparison {
    FileComparison::from_rows(complete_diff_rows(
        parse_unified_diff(diff),
        left_lines,
        right_lines,
        complete_empty,
    ))
}

pub(crate) fn text_preview_lines(bytes: Option<&[u8]>) -> Result<Vec<String>, String> {
    let Some(bytes) = bytes else {
        return Ok(Vec::new());
    };
    ensure_blob_text_previewable(bytes)?;
    Ok(lines_from_bytes(bytes))
}

pub(crate) fn lines_from_bytes(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn ensure_workdir_text_previewable(
    repo_path: &Path,
    file_path: &str,
) -> Result<(), String> {
    let _ = workdir_text_bytes(repo_path, file_path)?;
    Ok(())
}

pub(crate) fn ensure_tree_text_previewable(
    repo_path: &Path,
    rev: Option<&str>,
    file_path: &str,
) -> Result<(), String> {
    let Some(rev) = rev else {
        return Ok(());
    };
    let Some(bytes) = tree_file_bytes(repo_path, rev, file_path, MAX_TEXT_PREVIEW_BYTES)? else {
        return Ok(());
    };
    ensure_blob_text_previewable(&bytes)
}

pub(crate) fn workdir_text_bytes(
    repo_path: &Path,
    file_path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let path = repo_root(repo_path)?.join(file_path);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };

    if metadata.is_dir() {
        return Ok(None);
    }
    if metadata.len() as usize > MAX_TEXT_PREVIEW_BYTES {
        return Err(format!(
            "{} is too large to preview as text.",
            file_name(file_path)
        ));
    }

    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    ensure_blob_text_previewable(&bytes)?;
    Ok(Some(bytes))
}

pub(crate) fn ensure_blob_text_previewable(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_TEXT_PREVIEW_BYTES {
        return Err("File is too large to preview as text.".to_string());
    }
    if is_binary_bytes(bytes) {
        return Err("Binary files cannot be previewed as text.".to_string());
    }
    Ok(())
}

pub(crate) fn workdir_binary_bytes(
    repo_path: &Path,
    file_path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let path = repo_root(repo_path)?.join(file_path);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };

    if metadata.is_dir() {
        return Ok(None);
    }
    if metadata.len() as usize > MAX_BINARY_PREVIEW_BYTES {
        return Err(format!("{} is too large to preview.", file_name(file_path)));
    }

    std::fs::read(path).map(Some).map_err(|err| err.to_string())
}

pub(crate) fn tree_file_binary_bytes_opt(
    repo_path: &Path,
    rev: Option<&str>,
    file_path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let Some(rev) = rev else {
        return Ok(None);
    };
    tree_file_bytes(repo_path, rev, file_path, MAX_BINARY_PREVIEW_BYTES)
}

pub(crate) fn tree_file_bytes(
    repo_path: &Path,
    rev: &str,
    file_path: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let spec = format!("{rev}:{file_path}");
    if run_git(repo_path, &["cat-file", "-e", &spec]).is_err() {
        return Ok(None);
    }
    let size = run_git(repo_path, &["cat-file", "-s", &spec])?
        .trim()
        .parse::<usize>()
        .map_err(|err| format!("Failed to parse git object size: {err}"))?;
    if size > max_bytes {
        return Err(format!("{} is too large to preview.", file_name(file_path)));
    }

    run_git_bytes(repo_path, &["show", &spec]).map(Some)
}

pub(crate) fn is_binary_bytes(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

#[derive(Default)]
struct DiffRowsBuilder {
    rows: Vec<FileDiffRow>,
    deleted: Vec<PendingDiffLine>,
    added: Vec<PendingDiffLine>,
}

impl DiffRowsBuilder {
    fn push_context(
        &mut self,
        left_number: Option<usize>,
        right_number: Option<usize>,
        text: String,
    ) {
        self.flush();
        self.rows.push(FileDiffRow {
            left_number,
            right_number,
            left_text: Some(text.clone()),
            right_text: Some(text),
            left_kind: DiffKind::Context,
            right_kind: DiffKind::Context,
        });
    }

    fn push_deleted(&mut self, number: Option<usize>, text: String) {
        self.deleted.push(PendingDiffLine { number, text });
    }

    fn push_added(&mut self, number: Option<usize>, text: String) {
        self.added.push(PendingDiffLine { number, text });
    }

    fn flush(&mut self) {
        for index in 0..self.deleted.len().max(self.added.len()) {
            let deleted = self.deleted.get(index);
            let added = self.added.get(index);

            self.rows.push(FileDiffRow {
                left_number: deleted.and_then(|line| line.number),
                right_number: added.and_then(|line| line.number),
                left_text: deleted.map(|line| line.text.clone()),
                right_text: added.map(|line| line.text.clone()),
                left_kind: if deleted.is_some() {
                    DiffKind::Deleted
                } else {
                    DiffKind::Context
                },
                right_kind: if added.is_some() {
                    DiffKind::Added
                } else {
                    DiffKind::Context
                },
            });
        }

        self.deleted.clear();
        self.added.clear();
    }
}

struct PendingDiffLine {
    number: Option<usize>,
    text: String,
}

pub(crate) fn parse_unified_diff(diff: &str) -> Vec<FileDiffRow> {
    let mut builder = DiffRowsBuilder::default();
    let mut next_left = None::<usize>;
    let mut next_right = None::<usize>;
    let mut in_hunk = false;

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            builder.flush();
            next_left = None;
            next_right = None;
            in_hunk = false;
            continue;
        }
        if !in_hunk && is_unified_metadata_line(line) {
            builder.flush();
            continue;
        }
        if line.starts_with("@@") {
            builder.flush();
            if let Some((left, right)) = parse_hunk_line_numbers(line) {
                next_left = Some(left);
                next_right = Some(right);
            }
            in_hunk = true;
            continue;
        }
        if line.starts_with("\\ ") {
            continue;
        }
        if !in_hunk {
            continue;
        }

        if let Some(text) = line.strip_prefix('-') {
            let left_number = next_left;
            next_left = next_left.map(|number| number.saturating_add(1));
            builder.push_deleted(left_number, text.to_string());
        } else if let Some(text) = line.strip_prefix('+') {
            let right_number = next_right;
            next_right = next_right.map(|number| number.saturating_add(1));
            builder.push_added(right_number, text.to_string());
        } else {
            let text = line.strip_prefix(' ').unwrap_or(line).to_string();
            let left_number = next_left;
            let right_number = next_right;
            next_left = next_left.map(|number| number.saturating_add(1));
            next_right = next_right.map(|number| number.saturating_add(1));
            builder.push_context(left_number, right_number, text);
        }
    }

    builder.flush();
    builder.rows
}

pub(crate) fn is_unified_metadata_line(line: &str) -> bool {
    line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("new file mode ")
        || line.starts_with("copy from ")
        || line.starts_with("copy to ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("similarity index ")
        || line.starts_with("dissimilarity index ")
        || line.starts_with("Binary files ")
        || line.starts_with("GIT binary patch")
        || line.starts_with("literal ")
        || line.starts_with("delta ")
}

pub(crate) fn parse_hunk_line_numbers(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace();
    parts.next()?;
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((parse_hunk_start(old)?, parse_hunk_start(new)?))
}

pub(crate) fn parse_hunk_start(value: &str) -> Option<usize> {
    value
        .split_once(',')
        .map(|(start, _)| start)
        .unwrap_or(value)
        .parse::<usize>()
        .ok()
}
