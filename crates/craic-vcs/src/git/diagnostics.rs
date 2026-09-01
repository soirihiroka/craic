pub fn is_local_changes_overwritten_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let would_overwrite = lower.contains("would be overwritten")
        && (lower.contains("local changes")
            || lower.contains("untracked working tree files")
            || lower.contains("files would be overwritten"));
    let rebase_dirty = lower.contains("cannot pull with rebase")
        && (lower.contains("unstaged changes")
            || lower.contains("uncommitted changes")
            || lower.contains("please commit or stash"));
    let merge_dirty = lower.contains("commit your changes or stash them")
        && (lower.contains("merge") || lower.contains("pull"));

    would_overwrite || rebase_dirty || merge_dirty
}

pub fn parse_files_to_be_overwritten(message: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut in_files_list = false;
    for line in message.lines() {
        if in_files_list {
            if line.starts_with('\t') || line.starts_with("    ") {
                let file = line.trim();
                if !file.is_empty() {
                    files.push(file.to_string());
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            break;
        }

        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("error:")
            && lower.contains("would be overwritten")
            && lower.trim_end().ends_with(':')
        {
            in_files_list = true;
        }
    }
    files.sort();
    files.dedup();
    files
}

pub fn local_changes_overwritten_body(files: &[String], pull_before_push: bool) -> String {
    let action_name = if pull_before_push {
        "pull remote changes before pushing"
    } else {
        "pull"
    };
    let mut body = format!("Unable to {action_name} when changes are present on your branch.");
    if !files.is_empty() {
        body.push_str(" The following files would be overwritten:");
        for file in files.iter().take(12) {
            body.push_str("\n  ");
            body.push_str(file);
        }
        if files.len() > 12 {
            body.push_str(&format!("\n  ... and {} more", files.len() - 12));
        }
    }
    body.push_str("\n\nYou can stash your changes now and recover them afterwards.");
    body
}
