use crate::github::{
    self, GitHubAuthAccount, GitHubPublishRepositoryRequest, GitHubRepoMetadata,
    GitHubRepositoryOwner,
};
use crate::system::capabilities::github::GitHubAccess;
use crate::system::path::WorkspaceRef;

#[derive(Clone, Debug)]
pub(crate) struct SshGitHubAccess {
    workspace: WorkspaceRef,
}

impl SshGitHubAccess {
    pub(crate) fn new(workspace: WorkspaceRef) -> Self {
        Self { workspace }
    }
}

impl GitHubAccess for SshGitHubAccess {
    fn repo_metadata(
        &self,
        repo_slug: &str,
        remote_name: Option<&str>,
        remote_url: Option<&str>,
    ) -> Result<GitHubRepoMetadata, String> {
        log::debug!(
            "ssh github repo metadata start workspace={} repo={} remote={}",
            self.workspace.display_name,
            repo_slug,
            remote_url.unwrap_or_default()
        );
        github::repo_metadata_for_workspace(
            &self.workspace.id.to_string(),
            &self.workspace.root.absolute,
            repo_slug,
            remote_name,
            remote_url,
            || github::fetch_repo_metadata(repo_slug),
        )
    }

    fn open_pull_requests(&self) -> Result<Vec<github::PullRequestInfo>, String> {
        log::debug!(
            "ssh github pull requests start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
        github::open_pull_requests(&self.workspace.root.absolute)
    }

    fn authenticated_accounts(&self) -> Result<Vec<GitHubAuthAccount>, String> {
        log::debug!(
            "ssh github auth accounts start workspace={}",
            self.workspace.display_name
        );
        github::authenticated_accounts()
    }

    fn repository_owners(
        &self,
        account: &GitHubAuthAccount,
    ) -> Result<Vec<GitHubRepositoryOwner>, String> {
        log::debug!(
            "ssh github repository owners start workspace={} account={} host={}",
            self.workspace.display_name,
            account.login,
            account.host
        );
        github::repository_owners_for_account(account)
    }

    fn publish_repository(
        &self,
        _request: &GitHubPublishRepositoryRequest,
    ) -> Result<String, String> {
        Err("Publishing SSH workspaces with GitHub CLI is unavailable.".to_string())
    }

    fn repository_exists(&self, request: &GitHubPublishRepositoryRequest) -> Result<bool, String> {
        github::repository_exists(request)
    }
}
