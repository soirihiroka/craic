use super::GitHubAccess;
use crate::github::{
    self, GitHubAuthAccount, GitHubPublishRepositoryRequest, GitHubRepoMetadata,
    GitHubRepositoryOwner,
};
use crate::system::capabilities::shell::{
    ShellAccess, ShellCommandOutput, ShellCommandRunRequest, ShellRunRequest,
};
use crate::system::path::WorkspaceRef;
use std::sync::{Arc, mpsc};

#[derive(Clone)]
pub struct SshGitHubAccess {
    workspace: WorkspaceRef,
    shell: Arc<dyn ShellAccess>,
}

impl SshGitHubAccess {
    pub fn new(workspace: WorkspaceRef, shell: Arc<dyn ShellAccess>) -> Self {
        Self { workspace, shell }
    }

    fn run_gh(&self, operation: &str, args: Vec<String>) -> Result<ShellCommandOutput, String> {
        let gh = self.gh_path()?;
        let request =
            ShellCommandRunRequest::new(operation, self.workspace.root.clone(), gh).args(args);
        let output = self.run_command(request)?;
        if output.status_success(&[0]) {
            Ok(output)
        } else {
            Err(gh_failure(operation, output))
        }
    }

    fn run_gh_with_account(
        &self,
        operation: &str,
        account: &GitHubAuthAccount,
        args: &[String],
    ) -> Result<ShellCommandOutput, String> {
        let gh = self.gh_path()?;
        let script = github::gh_with_account_script(&gh, &account.host, &account.login, args);
        let request = ShellRunRequest::new(operation, self.workspace.root.clone(), script);
        let output = self.run_script(request)?;
        if output.status_success(&[0]) {
            Ok(output)
        } else {
            Err(gh_failure(operation, output))
        }
    }

    fn run_command(&self, request: ShellCommandRunRequest) -> Result<ShellCommandOutput, String> {
        let (sender, receiver) = mpsc::channel();
        self.shell.run_fast_command(
            request,
            Box::new(move |result| {
                let _ = sender.send(result);
            }),
        );
        receiver
            .recv()
            .map_err(|_| "Remote gh command did not return a result.".to_string())?
    }

    fn run_script(&self, request: ShellRunRequest) -> Result<ShellCommandOutput, String> {
        let (sender, receiver) = mpsc::channel();
        self.shell.run_fast_script(
            request,
            Box::new(move |result| {
                let _ = sender.send(result);
            }),
        );
        receiver
            .recv()
            .map_err(|_| "Remote gh script did not return a result.".to_string())?
    }

    fn gh_path(&self) -> Result<String, String> {
        self.shell
            .which("gh")?
            .ok_or_else(|| "gh was not found on the remote user shell path.".to_string())
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
            || {
                let output = self.run_gh(
                    "gh repo view",
                    vec![
                        "repo".to_string(),
                        "view".to_string(),
                        repo_slug.trim().to_string(),
                        "--json".to_string(),
                        "isFork,isPrivate".to_string(),
                        "--jq".to_string(),
                        github::repo_metadata_jq().to_string(),
                    ],
                )?;
                let value = output.stdout_text_trimmed();
                github::parse_repo_metadata_value(&value).ok_or_else(|| {
                    format!("Invalid gh repository metadata response for {repo_slug}: {value}")
                })
            },
        )
    }

    fn open_pull_requests(&self) -> Result<Vec<github::PullRequestInfo>, String> {
        log::debug!(
            "ssh github pull requests start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
        let output = self.run_gh(
            "gh pr list",
            vec![
                "pr".to_string(),
                "list".to_string(),
                "--state".to_string(),
                "open".to_string(),
                "--json".to_string(),
                "number,title,author,createdAt,isDraft,headRefName".to_string(),
            ],
        )?;
        github::parse_pull_requests(&output.stdout)
    }

    fn authenticated_accounts(&self) -> Result<Vec<GitHubAuthAccount>, String> {
        log::debug!(
            "ssh github auth accounts start workspace={}",
            self.workspace.display_name
        );
        let output = self.run_gh(
            "gh auth status",
            vec![
                "auth".to_string(),
                "status".to_string(),
                "--json".to_string(),
                "hosts".to_string(),
            ],
        )?;
        github::parse_authenticated_accounts(&output.stdout)
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
        let args = vec!["api".to_string(), "user/orgs".to_string()];
        match self.run_gh_with_account("gh user orgs", account, &args) {
            Ok(output) => github::repository_owners_from_orgs(account, &output.stdout),
            Err(err) => {
                log::warn!(
                    "failed to load ssh github publish owners account={} host={}: {err}",
                    account.login,
                    account.host
                );
                Ok(vec![GitHubRepositoryOwner {
                    host: account.host.clone(),
                    auth_login: account.login.clone(),
                    owner: account.login.clone(),
                }])
            }
        }
    }

    fn publish_repository(
        &self,
        _request: &GitHubPublishRepositoryRequest,
    ) -> Result<String, String> {
        Err("Publishing SSH workspaces with GitHub CLI is unavailable.".to_string())
    }

    fn repository_exists(&self, request: &GitHubPublishRepositoryRequest) -> Result<bool, String> {
        let host = request.host.trim();
        let auth_login = request.auth_login.trim();
        let owner = request.owner.trim();
        let name = request.name.trim();
        if host.is_empty() || auth_login.is_empty() || owner.is_empty() || name.is_empty() {
            return Ok(false);
        }

        let account = GitHubAuthAccount {
            host: host.to_string(),
            login: auth_login.to_string(),
        };
        let repo_slug = format!("{owner}/{name}");
        let args = vec![
            "repo".to_string(),
            "view".to_string(),
            repo_slug.clone(),
            "--json".to_string(),
            "name".to_string(),
        ];
        let gh = self.gh_path()?;
        let script = github::gh_with_account_script(&gh, &account.host, &account.login, &args);
        let output = self.run_script(ShellRunRequest::new(
            "gh repo exists",
            self.workspace.root.clone(),
            script,
        ))?;
        if output.status_success(&[0]) {
            return Ok(true);
        }

        let message = output.failure_message();
        let lower = message.to_lowercase();
        if lower.contains("could not resolve to a repository")
            || lower.contains("not found")
            || lower.contains("http 404")
        {
            Ok(false)
        } else {
            Err(if message.is_empty() {
                format!("gh repo view {repo_slug} failed.")
            } else {
                format!("gh repo view {repo_slug} failed: {message}")
            })
        }
    }
}

fn gh_failure(operation: &str, output: ShellCommandOutput) -> String {
    let message = output.failure_message();
    if message.is_empty() {
        format!("{operation} failed with status {:?}", output.status_code)
    } else {
        message
    }
}
