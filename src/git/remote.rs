use super::*;

pub fn remote_web_url(remote_url: &str) -> String {
    let trimmed = remote_url.trim_end_matches(".git");

    if let Some(path) = trimmed.strip_prefix("git@github.com:") {
        return format!("https://github.com/{path}");
    }

    trimmed.to_string()
}

pub fn github_slug_for_path(path: &Path) -> Option<String> {
    let root = repo_root(path).ok()?;
    let remote_name = upstream_remote(&root).or_else(|| {
        Some("origin".to_string()).filter(|remote| remote_url(&root, remote).is_some())
    })?;
    remote_url(&root, &remote_name).and_then(|url| parse_repo_slug_from_remote_url(&url))
}

pub fn remote_commit_web_url(remote_url: &str, hash: &str) -> String {
    format!("{}/commit/{hash}", remote_web_url(remote_url))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoMetadata {
    Fork,
    Private,
    Public,
    Folder,
}

pub fn get_repo_metadata_with(
    path: &Path,
    github_metadata: &dyn Fn(
        &str,
        Option<&str>,
        Option<&str>,
    ) -> Option<crate::github::GitHubRepoMetadata>,
    gitlab_metadata: &dyn Fn(
        &str,
        Option<&str>,
        Option<&str>,
    ) -> Option<crate::gitlab::GitLabRepoMetadata>,
    bitbucket_metadata: &dyn Fn(
        &str,
        Option<&str>,
        Option<&str>,
    ) -> Option<crate::bitbucket::BitbucketRepoMetadata>,
) -> RepoMetadata {
    let Ok(root) = repo_root(path) else {
        return RepoMetadata::Folder;
    };

    if remote_url(&root, "upstream").is_some() {
        return RepoMetadata::Fork;
    }

    let remote_name = upstream_remote(&root).or_else(|| {
        Some("origin".to_string()).filter(|remote| remote_url(&root, remote).is_some())
    });

    if let Some(name) = remote_name {
        if let Some(url) = remote_url(&root, &name) {
            if let Some(slug) = crate::github::parse_github_url(&url)
                && let Some(metadata) = github_metadata(&slug, Some(&name), Some(&url))
            {
                return match metadata {
                    crate::github::GitHubRepoMetadata::Fork => RepoMetadata::Fork,
                    crate::github::GitHubRepoMetadata::Private => RepoMetadata::Private,
                    crate::github::GitHubRepoMetadata::Public => RepoMetadata::Public,
                };
            }
            if let Some(slug) = crate::gitlab::parse_gitlab_url(&url)
                && let Some(metadata) = gitlab_metadata(&slug, Some(&name), Some(&url))
            {
                return match metadata {
                    crate::gitlab::GitLabRepoMetadata::Fork => RepoMetadata::Fork,
                    crate::gitlab::GitLabRepoMetadata::Private => RepoMetadata::Private,
                    crate::gitlab::GitLabRepoMetadata::Public => RepoMetadata::Public,
                };
            }
            if let Some(slug) = crate::bitbucket::parse_bitbucket_url(&url)
                && let Some(metadata) = bitbucket_metadata(&slug, Some(&name), Some(&url))
            {
                return match metadata {
                    crate::bitbucket::BitbucketRepoMetadata::Fork => RepoMetadata::Fork,
                    crate::bitbucket::BitbucketRepoMetadata::Private => RepoMetadata::Private,
                    crate::bitbucket::BitbucketRepoMetadata::Public => RepoMetadata::Public,
                };
            }
        }
    }

    RepoMetadata::Private
}

pub(crate) fn remote_owner_from_remote_url(remote_url: &str) -> Option<String> {
    parse_repo_slug_from_remote_url(remote_url)
        .and_then(|slug| slug.split('/').next().map(str::to_string))
}

pub(crate) fn parse_repo_slug_from_remote_url(remote_url: &str) -> Option<String> {
    github::parse_github_url(remote_url)
        .or_else(|| gitlab::parse_gitlab_url(remote_url))
        .or_else(|| bitbucket::parse_bitbucket_url(remote_url))
}
