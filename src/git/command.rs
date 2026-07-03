use super::*;

pub(crate) fn run_git(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn run_git_success(path: &Path, args: &[&str]) -> Result<bool, String> {
    Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map(|output| output.status.success())
        .map_err(|err| format!("Failed to run git: {err}"))
}

pub(crate) fn run_git_bytes(path: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn run_git_streaming_stderr(
    path: &Path,
    args: &[&str],
    mut progress: impl FnMut(&str),
) -> Result<String, String> {
    log::debug!(
        "running git command with streamed progress in {}: git {}",
        path.display(),
        args.join(" ")
    );

    let mut child = Command::new("git")
        .args(args)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture git output.".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture git progress.".to_string())?;

    let stdout_handle = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });

    let mut stderr_output = Vec::new();
    let mut pending_line = Vec::new();
    let mut buffer = [0u8; 1024];

    loop {
        let read = stderr
            .read(&mut buffer)
            .map_err(|err| format!("Failed to read git progress: {err}"))?;
        if read == 0 {
            break;
        }

        stderr_output.extend_from_slice(&buffer[..read]);
        for byte in &buffer[..read] {
            match *byte {
                b'\r' | b'\n' => {
                    emit_git_progress_line(&pending_line, &mut progress);
                    pending_line.clear();
                }
                byte => pending_line.push(byte),
            }
        }
    }
    emit_git_progress_line(&pending_line, &mut progress);

    let status = child
        .wait()
        .map_err(|err| format!("Failed to wait for git: {err}"))?;
    let stdout_output = stdout_handle
        .join()
        .map_err(|_| "Git output reader stopped unexpectedly.".to_string())?
        .map_err(|err| format!("Failed to read git output: {err}"))?;

    if status.success() {
        Ok(String::from_utf8_lossy(&stdout_output).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&stderr_output).trim().to_string();
        let stdout = String::from_utf8_lossy(&stdout_output).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn emit_git_progress_line(line: &[u8], progress: &mut impl FnMut(&str)) {
    let line = String::from_utf8_lossy(line);
    let line = line.trim();
    if !line.is_empty() {
        progress(line);
    }
}

pub(crate) fn git_fetch_progress_label(line: &str) -> Option<String> {
    let line = line.trim();
    let line = line.strip_prefix("remote:").unwrap_or(line).trim();
    let stages = [
        ("Enumerating objects", "Enumerating objects"),
        ("Counting objects", "Counting objects"),
        ("Compressing objects", "Compressing objects"),
        ("Receiving objects", "Receiving objects"),
        ("Resolving deltas", "Resolving deltas"),
    ];

    for (prefix, label) in stages {
        if !line.starts_with(prefix) {
            continue;
        }

        if let Some(percent) = progress_percent(line) {
            return Some(format!("{label} {percent}%"));
        }
        if line.contains("done") {
            return Some(format!("{label} done"));
        }
        return Some(label.to_string());
    }

    None
}

pub(crate) fn progress_percent(line: &str) -> Option<String> {
    let percent_index = line.find('%')?;
    let digits = line[..percent_index]
        .chars()
        .rev()
        .skip_while(|ch| ch.is_ascii_whitespace())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }

    Some(digits.chars().rev().collect())
}

pub(crate) fn run_git_owned(path: &Path, args: &[String]) -> Result<String, String> {
    run_git_owned_with_success_codes(path, args, &[0])
}

pub(crate) fn run_git_owned_untrimmed(path: &Path, args: &[String]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn run_git_owned_with_success_codes(
    path: &Path,
    args: &[String],
    success_codes: &[i32],
) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output
        .status
        .code()
        .is_some_and(|code| success_codes.contains(&code))
    {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn run_git_owned_with_stdin(
    path: &Path,
    args: &[String],
    stdin: &[u8],
) -> Result<String, String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to run git: {err}"))?;
    if let Some(mut child_stdin) = child.stdin.take() {
        child_stdin
            .write_all(stdin)
            .map_err(|err| format!("Failed to write git stdin: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to wait for git: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn run_git_bytes_owned_with_stdin_and_success_codes(
    path: &Path,
    args: &[String],
    stdin: &[u8],
    success_codes: &[i32],
) -> Result<Vec<u8>, String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to run git: {err}"))?;
    if let Some(mut child_stdin) = child.stdin.take() {
        child_stdin
            .write_all(stdin)
            .map_err(|err| format!("Failed to write git stdin: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to wait for git: {err}"))?;
    if output
        .status
        .code()
        .is_some_and(|code| success_codes.contains(&code))
    {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn run_git_owned_with_commit_timezone(
    path: &Path,
    args: &[String],
) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(path);
    configure_commit_timezone_env(path, &mut command)?;

    let output = command
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn run_git_owned_with_commit_timezone_and_stdin(
    path: &Path,
    args: &[String],
    stdin: &[u8],
) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_commit_timezone_env(path, &mut command)?;

    let mut child = command
        .spawn()
        .map_err(|err| format!("Failed to run git: {err}"))?;
    if let Some(mut child_stdin) = child.stdin.take() {
        child_stdin
            .write_all(stdin)
            .map_err(|err| format!("Failed to write git stdin: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to wait for git: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

pub(crate) fn configure_commit_timezone_env(
    path: &Path,
    command: &mut Command,
) -> Result<(), String> {
    let local_git_config = crate::workspace_config::git_config(path);
    let commit_timezone = local_git_config
        .commit_timezone
        .or_else(|| local_config_string(path, COMMIT_TIMEZONE_KEY));
    let use_system_timezone = local_git_config
        .use_system_timezone
        .unwrap_or_else(|| local_config_bool(path, USE_SYSTEM_TIMEZONE_KEY));
    let timezone = match commit_timezone {
        Some(timezone) => Some(crate::workspace_config::normalize_timezone(&timezone)?),
        None if use_system_timezone => None,
        None => Some(DEFAULT_COMMIT_TIMEZONE.to_string()),
    };

    if let Some(timezone) = timezone {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| err.to_string())?
            .as_secs();
        let git_date = format!("@{seconds} {timezone}");
        command.env("GIT_AUTHOR_DATE", &git_date);
        command.env("GIT_COMMITTER_DATE", git_date);
        log::debug!("using commit timezone {timezone}");
    } else {
        log::debug!("using system timezone for commit");
    }

    Ok(())
}
