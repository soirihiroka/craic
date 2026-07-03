use super::*;

pub fn initialize_repository(path: &Path) -> Result<String, String> {
    log::info!("git init start path={}", path.display());
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("init")
        .output()
        .map_err(|err| format!("Failed to run git init: {err}"))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        log::info!("git init complete path={}", path.display());
        if !stdout.is_empty() {
            Ok(stdout)
        } else if !stderr.is_empty() {
            Ok(stderr)
        } else {
            Ok("Initialized Git repository.".to_string())
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        log::warn!("git init failed path={} error={}", path.display(), message);
        Err(if message.is_empty() {
            "git init failed.".to_string()
        } else {
            message
        })
    }
}

pub fn clone_repository(remote_url: &str, destination: &Path) -> Result<String, String> {
    let remote_url = remote_url.trim();
    if remote_url.is_empty() {
        return Err("Remote Git source is required.".to_string());
    }

    log::info!("git clone start destination={}", destination.display());
    let output = Command::new("git")
        .arg("clone")
        .arg(remote_url)
        .arg(destination)
        .output()
        .map_err(|err| format!("Failed to run git clone: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        log::info!("git clone complete destination={}", destination.display());
        if !stdout.is_empty() {
            Ok(stdout)
        } else if !stderr.is_empty() {
            Ok(stderr)
        } else {
            Ok("Cloned repository.".to_string())
        }
    } else {
        let message = if stderr.is_empty() { stdout } else { stderr };
        log::warn!("git clone failed destination={}", destination.display());
        Err(if message.is_empty() {
            "git clone failed.".to_string()
        } else {
            message
        })
    }
}

pub fn commit_paths(
    path: &Path,
    summary: &str,
    description: &str,
    files: &[String],
) -> Result<String, String> {
    let summary = summary.trim();

    if summary.is_empty() {
        return Err("Commit summary is required.".to_string());
    }

    if files.is_empty() {
        return Err("Select at least one file to commit.".to_string());
    }

    let plan = commit_target_plan(path, files)?;
    if plan.force_remove_paths.is_empty() && plan.update_paths.is_empty() {
        return Err("Select at least one file to commit.".to_string());
    }
    reset_index_for_selected_commit(path)?;
    stage_commit_plan(path, &plan)?;

    run_git_owned_with_commit_timezone_and_stdin(
        path,
        &["commit".to_string(), "-F".to_string(), "-".to_string()],
        &commit_message_stdin(summary, description),
    )
}

struct CommitTargetPlan {
    force_remove_paths: Vec<String>,
    update_paths: Vec<String>,
}

fn commit_target_plan(path: &Path, selected_files: &[String]) -> Result<CommitTargetPlan, String> {
    let entries = status_entries(path)?;
    let mut force_remove_paths = Vec::<String>::new();
    let mut update_paths = Vec::<String>::new();
    let mut seen_force_remove_paths = HashSet::new();
    let mut seen_update_paths = HashSet::new();

    for requested in selected_files {
        let mut resolved = false;

        for entry in &entries {
            if !porcelain_entry_matches_path(entry, requested) {
                continue;
            }

            push_commit_target_paths(
                &mut force_remove_paths,
                &mut seen_force_remove_paths,
                porcelain_entry_force_remove_paths(entry),
            );
            push_commit_target_paths(
                &mut update_paths,
                &mut seen_update_paths,
                porcelain_entry_update_paths(entry),
            );

            resolved = true;
            break;
        }

        if !resolved {
            if seen_update_paths.insert(requested.clone()) {
                update_paths.push(requested.clone());
            }
        }
    }

    log::debug!(
        "resolved git commit targets selected_count={} force_remove_count={} update_count={}",
        selected_files.len(),
        force_remove_paths.len(),
        update_paths.len()
    );

    Ok(CommitTargetPlan {
        force_remove_paths,
        update_paths,
    })
}

fn push_commit_target_paths(
    target: &mut Vec<String>,
    seen: &mut HashSet<String>,
    paths: Vec<String>,
) {
    for path in paths {
        if seen.insert(path.clone()) {
            target.push(path);
        }
    }
}

fn reset_index_for_selected_commit(path: &Path) -> Result<(), String> {
    if run_git(path, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        run_git(path, &["reset", "--", "."]).map(|_| ())
    } else {
        run_git(path, &["rm", "--cached", "-r", "--ignore-unmatch", "."]).map(|_| ())
    }
}

fn stage_commit_plan(path: &Path, plan: &CommitTargetPlan) -> Result<(), String> {
    if !plan.force_remove_paths.is_empty() {
        update_index_paths(path, &["--force-remove"], &plan.force_remove_paths)?;
    }
    if !plan.update_paths.is_empty() {
        update_index_paths(
            path,
            &["--add", "--remove", "--replace"],
            &plan.update_paths,
        )?;
    }
    Ok(())
}

fn update_index_paths(path: &Path, options: &[&str], paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut args = vec!["update-index".to_string()];
    args.extend(options.iter().map(|option| option.to_string()));
    args.push("-z".to_string());
    args.push("--stdin".to_string());

    let mut stdin = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }

    run_git_owned_with_stdin(path, &args, &stdin).map(|_| ())
}

fn commit_message_stdin(summary: &str, description: &str) -> Vec<u8> {
    let mut message = summary.trim().to_string();
    let description = description.trim();
    if !description.is_empty() {
        message.push_str("\n\n");
        message.push_str(description);
    }
    message.push('\n');
    message.into_bytes()
}

pub fn discard_path(path: &Path, file_path: &str) -> Result<String, String> {
    let status = run_git(path, &["status", "--porcelain", "--", file_path])?;
    if status.lines().any(|line| line.starts_with("??")) {
        let target = path.join(file_path);
        if target.is_dir() {
            std::fs::remove_dir_all(&target).map_err(|err| err.to_string())?;
        } else if target.exists() {
            std::fs::remove_file(&target).map_err(|err| err.to_string())?;
        }
        return Ok(format!("Discarded {file_path}."));
    }

    run_git(
        path,
        &["restore", "--staged", "--worktree", "--", file_path],
    )?;
    Ok(format!("Discarded {file_path}."))
}

pub fn push(path: &Path) -> Result<String, String> {
    run_git(path, &["push"])
}

fn conflicted_files(repo_path: &Path) -> Vec<String> {
    let mut files = status_entries(repo_path)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.unmerged || entry.status_code.contains('U'))
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files
}

pub fn pull(path: &Path) -> Result<String, String> {
    match run_git(
        path,
        &[
            "-c",
            "rebase.backend=merge",
            "pull",
            "--ff",
            "--recurse-submodules",
        ],
    ) {
        Ok(output) => Ok(output),
        Err(err) => {
            let conflicts = conflicted_files(path);
            let _ = run_git(path, &["rebase", "--abort"]);
            let _ = run_git(path, &["merge", "--abort"]);
            if !conflicts.is_empty() {
                let file_list = conflicts
                    .iter()
                    .map(|file| format!("  • {file}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                Err(format!(
                    "Pull failed due to merge conflicts in the following files:\n\n{file_list}\n\nNote: The pull operation was aborted automatically to keep your repository in a safe state."
                ))
            } else {
                Err(format!(
                    "{err}\n\nNote: The pull operation was aborted automatically to keep your repository in a safe state."
                ))
            }
        }
    }
}

pub fn publish(path: &Path, remote: &str, branch: &str) -> Result<String, String> {
    run_git(path, &["push", "-u", remote, branch])
}

pub fn fetch_with_progress(
    path: &Path,
    remote: Option<&str>,
    mut progress: impl FnMut(String),
) -> Result<String, String> {
    let mut args = vec!["fetch", "--progress"];
    if let Some(remote) = remote {
        args.push(remote);
    }

    let mut last_progress = String::new();
    run_git_streaming_stderr(path, &args, |line| {
        if let Some(label) = git_fetch_progress_label(line)
            && label != last_progress
        {
            last_progress = label.clone();
            progress(label);
        }
    })
}

pub fn checkout_branch(path: &Path, branch: &str) -> Result<String, String> {
    run_git(path, &["checkout", branch])
}

pub fn checkout_remote_branch(
    path: &Path,
    remote_branch: &str,
    local_branch: &str,
) -> Result<String, String> {
    run_git(path, &["checkout", remote_branch, "-b", local_branch, "--"])
}

pub fn create_branch(path: &Path, branch: &str) -> Result<String, String> {
    run_git(path, &["checkout", "-b", branch])
}

pub fn checkout_commit(path: &Path, hash: &str) -> Result<String, String> {
    run_git(path, &["checkout", hash])
}

pub fn create_branch_at_commit(path: &Path, branch: &str, hash: &str) -> Result<String, String> {
    run_git(path, &["checkout", "-b", branch, hash])
}

pub fn create_tag(path: &Path, tag: &str, hash: &str) -> Result<String, String> {
    run_git(path, &["tag", tag, hash])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Mixed,
    Hard,
}

pub fn reset_to_commit(path: &Path, hash: &str, mode: ResetMode) -> Result<String, String> {
    let mode_arg = match mode {
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
    };
    run_git(path, &["reset", mode_arg, hash])
}

pub fn revert_commit(path: &Path, hash: &str) -> Result<String, String> {
    run_git(path, &["revert", "--no-edit", hash])
}

pub fn cherry_pick_commit(path: &Path, hash: &str) -> Result<String, String> {
    run_git(path, &["cherry-pick", hash])
}

pub fn amend_head(path: &Path, summary: &str, description: &str) -> Result<String, String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Err("Commit summary is required.".to_string());
    }

    let mut args = vec![
        "commit".to_string(),
        "--amend".to_string(),
        "-m".to_string(),
        summary.to_string(),
    ];
    let description = description.trim();
    if !description.is_empty() {
        args.push("-m".to_string());
        args.push(description.to_string());
    }

    run_git_owned_with_commit_timezone(path, &args)
}

pub fn stash_changes(path: &Path) -> Result<String, String> {
    run_git(path, &["stash", "-u"])
}

pub fn pop_stash(path: &Path) -> Result<String, String> {
    run_git(path, &["stash", "pop"])
}
