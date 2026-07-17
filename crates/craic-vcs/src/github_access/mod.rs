mod local;
mod ssh;

use crate::github::{
    GitHubAuthAccount, GitHubPublishRepositoryRequest, GitHubRepoMetadata, GitHubRepositoryOwner,
    PullRequestInfo,
};
use crate::system::capabilities::shell::ShellAccess;
use crate::system::{
    ProviderKind, SystemId, SystemProvider, SystemProviderRegistry, SystemRef, WorkspaceRef,
};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use local::LocalGitHubAccess;
use ssh::SshGitHubAccess;

pub trait GitHubAccess: Send + Sync {
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

pub fn github_access(
    providers: &SystemProviderRegistry,
    system: &SystemRef,
    workspace: &WorkspaceRef,
) -> Option<Arc<dyn GitHubAccess>> {
    let shell = providers.shell(&system.id, workspace)?;
    cached_github_access(&system.id, system.provider_kind, workspace, shell)
}

pub fn github_access_for_provider(
    provider: &dyn SystemProvider,
    workspace: &WorkspaceRef,
) -> Option<Arc<dyn GitHubAccess>> {
    let shell = provider.shell(workspace)?;
    cached_github_access(&provider.id(), provider.kind(), workspace, shell)
}

fn cached_github_access(
    system_id: &SystemId,
    provider_kind: ProviderKind,
    workspace: &WorkspaceRef,
    shell: Arc<dyn ShellAccess>,
) -> Option<Arc<dyn GitHubAccess>> {
    static CACHE: OnceLock<RwLock<HashMap<String, Arc<dyn GitHubAccess>>>> = OnceLock::new();

    let key = format!("{}|{}", system_id, workspace.id);
    let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(access) = cache
        .read()
        .expect("github access cache poisoned")
        .get(&key)
        .cloned()
    {
        return Some(access);
    }

    let access: Arc<dyn GitHubAccess> = match provider_kind {
        ProviderKind::Local => Arc::new(LocalGitHubAccess::new(workspace.clone(), shell)),
        ProviderKind::Ssh => Arc::new(SshGitHubAccess::new(workspace.clone(), shell)),
        ProviderKind::Container => return None,
    };
    log::debug!(
        "creating github access provider={} workspace={} root={}",
        system_id,
        workspace.display_name,
        workspace.root.absolute
    );
    cache
        .write()
        .expect("github access cache poisoned")
        .insert(key, access.clone());
    Some(access)
}
