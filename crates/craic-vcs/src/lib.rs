pub use craic_config as config;
pub use craic_project::workspace_config;
pub use craic_system::system;

pub mod bitbucket;
pub mod git;
pub mod github;
pub mod github_access;
pub mod gitignore;
pub mod gitlab;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMessageContext {
    pub repo_name: String,
    pub branch: String,
    pub files: Vec<String>,
    pub statuses: String,
    pub diff: String,
    pub commit_convention: Option<String>,
}

pub use github_access::{GitHubAccess, github_access, github_access_for_provider};
