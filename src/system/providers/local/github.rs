use crate::git;
use crate::github::{
    self, GitHubAuthAccount, GitHubPublishRepositoryRequest, GitHubRepoMetadata,
    GitHubRepositoryOwner,
};
use crate::system::capabilities::github::GitHubAccess;
use crate::system::path::WorkspaceRef;
use std::path::Path;

#[derive(Clone, Debug)]
pub(crate) struct LocalGitHubAccess {
    workspace: WorkspaceRef,
}

impl LocalGitHubAccess {
    pub(crate) fn new(workspace: WorkspaceRef) -> Self {
        Self { workspace }
    }

    fn saved_auth_account(&self) -> Option<GitHubAuthAccount> {
        git::settings(Path::new(&self.workspace.root.absolute)).github_auth_account
    }
}

impl GitHubAccess for LocalGitHubAccess {
    fn preferred_auth_account(&self) -> Option<GitHubAuthAccount> {
        self.saved_auth_account()
    }

    fn repo_metadata(
        &self,
        repo_slug: &str,
        remote_name: Option<&str>,
        remote_url: Option<&str>,
    ) -> Result<GitHubRepoMetadata, String> {
        log::debug!(
            "local github repo metadata start workspace={} repo={} remote={}",
            self.workspace.display_name,
            repo_slug,
            remote_url.unwrap_or_default()
        );
        let auth_account = self.saved_auth_account();
        github::repo_metadata_for_workspace(
            &self.workspace.id.to_string(),
            &self.workspace.root.absolute,
            repo_slug,
            remote_name,
            remote_url,
            || github::fetch_repo_metadata_with_account(repo_slug, auth_account.as_ref()),
        )
    }

    fn open_pull_requests(&self) -> Result<Vec<github::PullRequestInfo>, String> {
        log::debug!(
            "local github pull requests start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
        let auth_account = self.saved_auth_account();
        github::open_pull_requests_with_account(
            &self.workspace.root.absolute,
            auth_account.as_ref(),
        )
    }

    fn authenticated_accounts(&self) -> Result<Vec<GitHubAuthAccount>, String> {
        log::debug!(
            "local github auth accounts start workspace={}",
            self.workspace.display_name
        );
        github::authenticated_accounts()
    }

    fn repository_owners(
        &self,
        account: &GitHubAuthAccount,
    ) -> Result<Vec<GitHubRepositoryOwner>, String> {
        log::debug!(
            "local github repository owners start workspace={} account={} host={}",
            self.workspace.display_name,
            account.login,
            account.host
        );
        github::repository_owners_for_account(account)
    }

    fn publish_repository(
        &self,
        request: &GitHubPublishRepositoryRequest,
    ) -> Result<String, String> {
        log::info!(
            "local github publish repository start workspace={} owner={} name={}",
            self.workspace.display_name,
            request.owner,
            request.name
        );
        github::publish_repository(&self.workspace.root.absolute, request)
    }

    fn repository_exists(&self, request: &GitHubPublishRepositoryRequest) -> Result<bool, String> {
        log::debug!(
            "local github repository exists start workspace={} owner={} name={}",
            self.workspace.display_name,
            request.owner,
            request.name
        );
        github::repository_exists(request)
    }
}
