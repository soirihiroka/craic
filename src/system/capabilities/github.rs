use crate::github::{
    GitHubAuthAccount, GitHubPublishRepositoryRequest, GitHubRepoMetadata, GitHubRepositoryOwner,
    PullRequestInfo,
};

pub(crate) trait GitHubAccess: Send + Sync {
    fn preferred_auth_account(&self) -> Option<GitHubAuthAccount> {
        None
    }

    fn repo_metadata(
        &self,
        repo_slug: &str,
        remote_name: Option<&str>,
        remote_url: Option<&str>,
    ) -> Result<GitHubRepoMetadata, String>;
    fn open_pull_requests(&self) -> Result<Vec<PullRequestInfo>, String>;
    fn authenticated_accounts(&self) -> Result<Vec<GitHubAuthAccount>, String>;
    fn repository_owners(
        &self,
        account: &GitHubAuthAccount,
    ) -> Result<Vec<GitHubRepositoryOwner>, String>;
    fn repository_exists(&self, request: &GitHubPublishRepositoryRequest) -> Result<bool, String>;
    fn publish_repository(
        &self,
        request: &GitHubPublishRepositoryRequest,
    ) -> Result<String, String>;
}
