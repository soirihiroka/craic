pub fn remote_web_url(remote_url: &str) -> String {
    let trimmed = remote_url.trim_end_matches(".git");

    if let Some(path) = trimmed.strip_prefix("git@github.com:") {
        return format!("https://github.com/{path}");
    }

    trimmed.to_string()
}

pub fn remote_commit_web_url(remote_url: &str, hash: &str) -> String {
    format!("{}/commit/{hash}", remote_web_url(remote_url))
}
