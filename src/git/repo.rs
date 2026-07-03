use super::*;

pub fn snapshot(path: &Path) -> Result<RepositorySnapshot, String> {
    let root = repo_root(path)?;

    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Repository")
        .to_string();
    let branch = current_branch(&root)?;

    let remote_name = upstream_remote(&root).or_else(|| {
        Some("origin".to_string()).filter(|remote| remote_url(&root, remote).is_some())
    });
    let remote_url = remote_name
        .as_deref()
        .and_then(|remote| remote_url(&root, remote));
    let remote_owner = remote_url.as_deref().and_then(remote_owner_from_remote_url);

    let (ahead, behind, has_upstream) = ahead_behind_count(&root);

    let last_fetch_at = last_fetch_at(&root);
    let user_name = config_string(&root, "user.name");
    let user_email = config_string(&root, "user.email");
    let github_avatar_url = user_email
        .as_deref()
        .and_then(github::login_from_noreply_email)
        .map(|login| github::avatar_url_for_login(&login));

    let branches = branches(&root, remote_name.as_deref())?;

    let warn_if_remote_owner_mismatch =
        local_config_bool_with_default(path, SHOW_REMOTE_OWNER_WARNING_KEY, true);

    let changed_files = changed_files(&root)?;

    let history_head = history_head(&root);

    Ok(RepositorySnapshot {
        name,
        branch,
        branches,
        remote_name,
        remote_url,
        remote_owner,
        ahead,
        behind,
        has_upstream,
        last_fetch_at,
        user_name,
        user_email,
        github_avatar_url,
        warn_if_remote_owner_mismatch,
        changed_files,
        history_head,
    })
}

pub fn settings(path: &Path) -> GitSettings {
    let local_user_name = local_config_string(path, "user.name");
    let local_user_email = local_config_string(path, "user.email");
    let local_git_config = crate::workspace_config::git_config(path);

    GitSettings {
        global_user_name: global_config_string("user.name"),
        global_user_email: global_config_string("user.email"),
        use_global_user: local_user_name.is_none() && local_user_email.is_none(),
        local_user_name,
        local_user_email,
        commit_timezone: local_git_config
            .commit_timezone
            .or_else(|| local_config_string(path, COMMIT_TIMEZONE_KEY)),
        warn_if_remote_owner_mismatch: local_git_config
            .warn_if_remote_owner_mismatch
            .unwrap_or_else(|| {
                local_config_bool_with_default(path, SHOW_REMOTE_OWNER_WARNING_KEY, true)
            }),
        use_system_timezone: local_git_config
            .use_system_timezone
            .unwrap_or_else(|| local_config_bool(path, USE_SYSTEM_TIMEZONE_KEY)),
        github_auth_account: local_git_config.github_auth_account,
    }
}

pub fn save_settings(
    path: &Path,
    use_global_user: bool,
    user_name: &str,
    user_email: &str,
    commit_timezone: &str,
    warn_if_remote_owner_mismatch: bool,
    use_system_timezone: bool,
    github_auth_account: Option<&github::GitHubAuthAccount>,
) -> Result<(), String> {
    if use_global_user {
        unset_local_config(path, "user.name")?;
        unset_local_config(path, "user.email")?;
    } else {
        set_local_config(path, "user.name", user_name.trim())?;
        set_local_config(path, "user.email", user_email.trim())?;
    }

    crate::workspace_config::save_git_config(
        path,
        commit_timezone,
        warn_if_remote_owner_mismatch,
        use_system_timezone,
        github_auth_account,
    )?;

    let _ = unset_local_config(path, COMMIT_TIMEZONE_KEY);
    let _ = unset_local_config(path, USE_SYSTEM_TIMEZONE_KEY);
    let _ = unset_local_config(path, SHOW_REMOTE_OWNER_WARNING_KEY);

    Ok(())
}

pub fn save_author_email(path: &Path, email: &str) -> Result<(), String> {
    let email = email.trim();
    if email.is_empty() {
        return Err("Author email is required.".to_string());
    }

    set_local_config(path, "user.email", email)
}

pub fn root_for_path(path: &Path) -> Option<PathBuf> {
    repo_root(path)
        .ok()
        .map(|root| root.canonicalize().unwrap_or(root))
}

pub(crate) fn repo_root(path: &Path) -> Result<PathBuf, String> {
    let root = run_git(path, &["rev-parse", "--show-toplevel"])?;
    if root.is_empty() {
        return Err("Bare repositories are not supported.".to_string());
    }
    Ok(PathBuf::from(root))
}

pub(crate) fn git_dir(path: &Path) -> Option<PathBuf> {
    let output = run_git(path, &["rev-parse", "--absolute-git-dir"]).ok()?;
    (!output.is_empty()).then(|| PathBuf::from(output))
}

pub(crate) fn current_branch(root: &Path) -> Result<String, String> {
    if let Ok(branch) = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        if !branch.is_empty() && branch != "HEAD" {
            return Ok(branch);
        }
    }

    if let Ok(hash) = run_git(root, &["rev-parse", "--short", "HEAD"]) {
        if !hash.is_empty() {
            return Ok(hash);
        }
    }

    Ok(local_config_string(root, "init.defaultBranch").unwrap_or_else(|| "main".to_string()))
}

pub(crate) fn branches(root: &Path, remote_name: Option<&str>) -> Result<Vec<BranchInfo>, String> {
    let current = current_branch(root).unwrap_or_default();
    let mut branches = Vec::new();
    let output = run_git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)%00%(refname)%00%(upstream:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;

    for line in output.lines() {
        let mut fields = line.split('\0');
        let Some(name) = fields.next().filter(|name| !name.is_empty()) else {
            continue;
        };
        let refname = fields.next().unwrap_or_default();
        let upstream = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        if name.ends_with("/HEAD") {
            continue;
        }
        let kind = if refname.starts_with("refs/remotes/") {
            BranchKind::Remote
        } else {
            BranchKind::Local
        };

        branches.push(BranchInfo {
            name: name.to_string(),
            is_current: kind == BranchKind::Local && name == current,
            kind,
            upstream,
            is_default: false,
            is_recent: false,
        });
    }

    let default_name = default_branch_name(root, remote_name);
    let remote_ref = remote_name
        .and_then(|remote| remote_head(root, remote).map(|head| format!("{remote}/{head}")));
    if let Some(index) = find_default_branch_index(&branches, &default_name, remote_ref.as_deref())
    {
        branches[index].is_default = true;
    }

    let mut by_name = HashMap::new();
    for (index, branch) in branches.iter().enumerate() {
        if branch.kind == BranchKind::Local {
            by_name.insert(branch.name.clone(), index);
        }
    }
    for name in recent_branch_names(root, RECENT_BRANCHES_LIMIT + 1) {
        if let Some(index) = by_name.get(&name).copied()
            && !branches[index].is_default
        {
            branches[index].is_recent = true;
        }
    }

    branches.sort_by(|left, right| match (left.is_current, right.is_current) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.name.cmp(&right.name),
    });

    Ok(branches)
}

pub(crate) fn default_branch_name(root: &Path, remote_name: Option<&str>) -> String {
    remote_name
        .and_then(|remote| remote_head(root, remote))
        .or_else(|| local_config_string(root, "init.defaultBranch"))
        .unwrap_or_else(|| "main".to_string())
}

pub(crate) fn remote_head(root: &Path, remote_name: &str) -> Option<String> {
    let target = run_git(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            &format!("refs/remotes/{remote_name}/HEAD"),
        ],
    )
    .ok()?;
    target
        .strip_prefix(&format!("{remote_name}/"))
        .map(ToString::to_string)
}

pub(crate) fn find_default_branch_index(
    branches: &[BranchInfo],
    default_name: &str,
    remote_ref: Option<&str>,
) -> Option<usize> {
    let mut local_hit = None;
    let mut local_tracking_hit = None;
    let mut remote_hit = None;

    for (index, branch) in branches.iter().enumerate() {
        match branch.kind {
            BranchKind::Local => {
                if branch.name == default_name {
                    local_hit = Some(index);
                }
                if remote_ref
                    .is_some_and(|remote_ref| branch.upstream.as_deref() == Some(remote_ref))
                    && (local_tracking_hit.is_none() || branch.name == default_name)
                {
                    local_tracking_hit = Some(index);
                }
            }
            BranchKind::Remote => {
                if remote_ref.is_some_and(|remote_ref| branch.name == remote_ref) {
                    remote_hit = Some(index);
                }
            }
        }
    }

    local_tracking_hit.or(local_hit).or(remote_hit)
}

pub(crate) fn recent_branch_names(root: &Path, limit: usize) -> Vec<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "log",
            "-g",
            "--no-abbrev-commit",
            "--pretty=oneline",
            "HEAD",
            "-n",
            "2500",
            "--",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    parse_recent_branch_names(&String::from_utf8_lossy(&output.stdout), limit)
}

pub(crate) fn parse_recent_branch_names(output: &str, limit: usize) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    let mut excluded = HashSet::new();

    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        let is_renamed = lower.contains(" renamed ");
        let Some((from, to)) = line
            .split_once(": moving from ")
            .or_else(|| line.split_once(" renamed "))
            .and_then(|(_, rest)| rest.rsplit_once(" to "))
        else {
            continue;
        };
        let from = from.trim().trim_start_matches("refs/heads/");
        let to = to.trim().trim_start_matches("refs/heads/");
        if is_renamed {
            excluded.insert(from.to_string());
        }
        if !to.is_empty() && !excluded.contains(to) && seen.insert(to.to_string()) {
            names.push(to.to_string());
        }
        if names.len() >= limit {
            break;
        }
    }

    names
}

pub(crate) fn upstream_remote(root: &Path) -> Option<String> {
    run_git(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok()
    .and_then(|upstream| upstream.split('/').next().map(ToString::to_string))
    .filter(|remote| !remote.is_empty())
}

pub(crate) fn remote_url(root: &Path, remote_name: &str) -> Option<String> {
    run_git(root, &["remote", "get-url", remote_name])
        .ok()
        .filter(|url| !url.is_empty())
}

pub(crate) fn config_string(path: &Path, key: &str) -> Option<String> {
    run_git(path, &["config", "--get", key])
        .ok()
        .filter(|value| !value.is_empty())
}

pub(crate) fn ahead_behind_count(root: &Path) -> (u32, u32, bool) {
    let Ok(out) = run_git(
        root,
        &[
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...@{upstream}",
            "--",
        ],
    ) else {
        return (0, 0, false);
    };
    let mut parts = out.split_whitespace();
    let ahead = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let behind = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    (ahead, behind, true)
}

pub(crate) fn last_fetch_at(root: &Path) -> Option<SystemTime> {
    git_dir(root)
        .and_then(|dir| std::fs::metadata(dir.join("FETCH_HEAD")).ok())
        .and_then(|metadata| metadata.modified().ok())
}

pub(crate) fn local_config_string(path: &Path, key: &str) -> Option<String> {
    run_git(path, &["config", "--local", "--get", key])
        .ok()
        .filter(|value| !value.is_empty())
}

pub(crate) fn local_config_bool(path: &Path, key: &str) -> bool {
    run_git(path, &["config", "--local", "--bool", "--get", key])
        .ok()
        .is_some_and(|value| value == "true")
}

pub(crate) fn local_config_bool_with_default(path: &Path, key: &str, default: bool) -> bool {
    run_git(path, &["config", "--local", "--bool", "--get", key])
        .ok()
        .and_then(|value| {
            if value == "true" {
                Some(true)
            } else if value == "false" {
                Some(false)
            } else {
                None
            }
        })
        .unwrap_or(default)
}

pub(crate) fn global_config_string(key: &str) -> Option<String> {
    Command::new("git")
        .args(["config", "--global", "--get", key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn set_local_config(path: &Path, key: &str, value: &str) -> Result<(), String> {
    run_git(path, &["config", "--local", key, value]).map(|_| ())
}

pub(crate) fn unset_local_config(path: &Path, key: &str) -> Result<(), String> {
    let output = Command::new("git")
        .args(["config", "--local", "--unset", key])
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() || output.status.code() == Some(5) {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn changed_files(root: &Path) -> Result<Vec<ChangedFile>, String> {
    let mut files = status_entries(root)?
        .iter()
        .filter(|entry| status_entry_visible(entry))
        .map(|entry| {
            let mut file = changed_file_from_porcelain_entry(entry);
            if file.status == "M" && deletion_only_change(root, &file.path) {
                file.status = "M-".to_string();
            }
            file.worktree_signature = changed_file_worktree_signature(root, &file.path);
            file
        })
        .collect::<Vec<_>>();

    sort_changed_files(&mut files);
    Ok(files)
}

pub(crate) fn status_entries(path: &Path) -> Result<Vec<GitStatusEntry>, String> {
    let output = run_git_bytes(
        path,
        &[
            "--no-optional-locks",
            "status",
            "--untracked-files=all",
            "--branch",
            "--porcelain=2",
            "-z",
        ],
    )?;
    Ok(parse_porcelain_status_entries(&output))
}

pub(crate) fn changed_file_worktree_signature(
    root: &Path,
    file_path: &str,
) -> Option<ChangedFileSignature> {
    let metadata = std::fs::symlink_metadata(root.join(file_path)).ok()?;
    Some(ChangedFileSignature {
        is_dir: metadata.is_dir(),
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

pub(crate) fn deletion_only_change(repo_path: &Path, file_path: &str) -> bool {
    let Ok(output) = run_git(repo_path, &["diff", "--numstat", "HEAD", "--", file_path]) else {
        return false;
    };
    let mut insertions = 0;
    let mut deletions = 0;

    for line in output.lines() {
        let mut fields = line.split('\t');
        let (Some(added), Some(deleted)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(added), Ok(deleted)) = (added.parse::<usize>(), deleted.parse::<usize>()) else {
            return false;
        };
        insertions += added;
        deletions += deleted;
    }

    insertions == 0 && deletions > 0
}

pub(crate) fn sort_changed_files(files: &mut [ChangedFile]) {
    files.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.status.cmp(&right.status))
    });
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

pub(crate) fn parse_porcelain_changed_entry(field: &str) -> Option<GitStatusEntry> {
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

pub(crate) fn parse_porcelain_renamed_entry(
    field: &str,
    old_path: Option<String>,
) -> Option<GitStatusEntry> {
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

pub(crate) fn parse_porcelain_unmerged_entry(field: &str) -> Option<GitStatusEntry> {
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

pub(crate) fn porcelain_entry_status_label(entry: &GitStatusEntry) -> &'static str {
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

pub(crate) fn ssh_workspace_list_script() -> &'static str {
    "emit_workspace() { \
       kind=$1; source=$2; d=$3; remote=''; url=''; \
       if git -C \"$d\" rev-parse --is-inside-work-tree >/dev/null 2>&1; then \
         remote=$(git -C \"$d\" rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null | sed 's#/.*##' || true); \
         if [ -z \"$remote\" ] && git -C \"$d\" remote get-url origin >/dev/null 2>&1; then remote=origin; fi; \
         if [ -z \"$remote\" ]; then remote=$(git -C \"$d\" remote 2>/dev/null | head -n 1); fi; \
         if [ -n \"$remote\" ]; then url=$(git -C \"$d\" remote get-url \"$remote\" 2>/dev/null || true); fi; \
         if [ -z \"$url\" ]; then url='-'; fi; \
       fi; \
       printf '%s\\t%s\\t%s\\t%s\\t%s\\n' \"$kind\" \"$source\" \"$d\" \"$remote\" \"$url\"; \
     }; \
     resolve_path() { \
       p=$1; \
       if [ \"$p\" = '~' ]; then printf '%s\\n' \"$HOME\"; \
       else case \"$p\" in \"~/\"*) printf '%s/%s\\n' \"$HOME\" \"${p#\\~/}\" ;; *) printf '%s\\n' \"$p\" ;; esac; fi; \
     }; \
     while IFS='	' read -r kind raw_path; do \
       [ -n \"$kind\" ] || continue; \
       path=$(resolve_path \"$raw_path\"); \
       case \"$kind\" in \
         W) [ -d \"$path\" ] && emit_workspace W \"$raw_path\" \"$path\" ;; \
         R) [ -d \"$path\" ] && find \"$path\" -mindepth 1 -maxdepth 1 -type d -print | while IFS= read -r d; do emit_workspace R \"$raw_path\" \"$d\"; done ;; \
       esac; \
     done"
}
