use super::GitHubAccess;
use crate::github::{
    self, GitHubAuthAccount, GitHubPublishRepositoryRequest, GitHubRepoMetadata,
    GitHubRepositoryOwner,
};
use crate::system::capabilities::shell::{
    ShellAccess, ShellCommandOutput, ShellCommandRunRequest, ShellRunRequest,
};
use crate::system::path::WorkspaceRef;
use std::path::Path;
use std::sync::{Arc, mpsc};

#[derive(Clone)]
pub struct LocalGitHubAccess {
    workspace: WorkspaceRef,
    shell: Arc<dyn ShellAccess>,
}

impl LocalGitHubAccess {
    pub fn new(workspace: WorkspaceRef, shell: Arc<dyn ShellAccess>) -> Self {
        Self { workspace, shell }
    }

    fn saved_auth_account(&self) -> Option<GitHubAuthAccount> {
        crate::workspace_config::git_config(Path::new(&self.workspace.root.absolute))
            .github_auth_account
    }

    fn run_gh(&self, operation: &str, args: Vec<String>) -> Result<ShellCommandOutput, String> {
        let gh = self.gh_path()?;
        let request =
            ShellCommandRunRequest::new(operation, self.workspace.root.clone(), gh).args(args);
        let output = self.run_command(request)?;
        if output.status_success(&[0]) {
            Ok(output)
        } else {
            let message = output.failure_message();
            Err(if message.is_empty() {
                format!("{operation} failed with status {:?}", output.status_code)
            } else {
                message
            })
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
            let message = output.failure_message();
            Err(if message.is_empty() {
                format!("{operation} failed with status {:?}", output.status_code)
            } else {
                message
            })
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
            .map_err(|_| "Local gh command did not return a result.".to_string())?
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
            .map_err(|_| "Local gh script did not return a result.".to_string())?
    }

    fn gh_path(&self) -> Result<String, String> {
        self.shell
            .which("gh")?
            .ok_or_else(|| "gh was not found on the user shell path.".to_string())
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
            || {
                let args = vec![
                    "repo".to_string(),
                    "view".to_string(),
                    repo_slug.trim().to_string(),
                    "--json".to_string(),
                    "isFork,isPrivate".to_string(),
                    "--jq".to_string(),
                    github::repo_metadata_jq().to_string(),
                ];
                let output = if let Some(account) = auth_account.as_ref() {
                    self.run_gh_with_account("gh repo view", account, &args)?
                } else {
                    self.run_gh("gh repo view", args)?
                };
                let value = output.stdout_text_trimmed();
                github::parse_repo_metadata_value(&value).ok_or_else(|| {
                    format!("Invalid gh repository metadata response for {repo_slug}: {value}")
                })
            },
        )
    }

    fn open_pull_requests(&self) -> Result<Vec<github::PullRequestInfo>, String> {
        log::debug!(
            "local github pull requests start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
        let auth_account = self.saved_auth_account();
        let args = vec![
            "pr".to_string(),
            "list".to_string(),
            "--state".to_string(),
            "open".to_string(),
            "--json".to_string(),
            "number,title,author,createdAt,isDraft,headRefName".to_string(),
        ];
        let output = if let Some(account) = auth_account.as_ref() {
            self.run_gh_with_account("gh pr list", account, &args)?
        } else {
            self.run_gh("gh pr list", args)?
        };
        github::parse_pull_requests(&output.stdout)
    }

    fn authenticated_accounts(&self) -> Result<Vec<GitHubAuthAccount>, String> {
        log::debug!(
            "local github auth accounts start workspace={}",
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
            "local github repository owners start workspace={} account={} host={}",
            self.workspace.display_name,
            account.login,
            account.host
        );
        let args = vec!["api".to_string(), "user/orgs".to_string()];
        match self.run_gh_with_account("gh user orgs", account, &args) {
            Ok(output) => github::repository_owners_from_orgs(account, &output.stdout),
            Err(err) => {
                log::warn!(
                    "failed to load local github publish owners account={} host={}: {err}",
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
        request: &GitHubPublishRepositoryRequest,
    ) -> Result<String, String> {
        log::info!(
            "local github publish repository start workspace={} owner={} name={}",
            self.workspace.display_name,
            request.owner,
            request.name
        );
        let host = request.host.trim();
        let auth_login = request.auth_login.trim();
        let owner = request.owner.trim();
        let name = request.name.trim();
        if host.is_empty() {
            return Err("GitHub host is required.".to_string());
        }
        if auth_login.is_empty() {
            return Err("GitHub account is required.".to_string());
        }
        if owner.is_empty() {
            return Err("Repository owner is required.".to_string());
        }
        if name.is_empty() {
            return Err("Repository name is required.".to_string());
        }
        if self.repository_exists(request)? {
            return Err(format!(
                "Repository {owner}/{name} already exists on {host}."
            ));
        }

        let account = GitHubAuthAccount {
            host: host.to_string(),
            login: auth_login.to_string(),
        };
        let repo_slug = format!("{owner}/{name}");
        let mut args = vec![
            "repo".to_string(),
            "create".to_string(),
            repo_slug.clone(),
            "--source".to_string(),
            self.workspace.root.absolute.clone(),
            "--remote".to_string(),
            "origin".to_string(),
        ];
        if request.has_commits {
            args.push("--push".to_string());
        } else {
            log::info!(
                "creating empty github repository without initial push workspace={} repo={repo_slug}",
                self.workspace.display_name
            );
        }
        args.push(if request.private {
            "--private".to_string()
        } else {
            "--public".to_string()
        });
        let output = self.run_gh_with_account("gh repo create", &account, &args)?;
        let stdout = output.stdout_text_trimmed();
        let stderr = output.stderr_text_trimmed();
        if !stdout.is_empty() {
            Ok(stdout)
        } else if !stderr.is_empty() {
            Ok(stderr)
        } else {
            Ok(format!("Published {repo_slug}."))
        }
    }

    fn repository_exists(&self, request: &GitHubPublishRepositoryRequest) -> Result<bool, String> {
        log::debug!(
            "local github repository exists start workspace={} owner={} name={}",
            self.workspace.display_name,
            request.owner,
            request.name
        );
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
        let output = match self.run_gh_with_account("gh repo exists", &account, &args) {
            Ok(_) => return Ok(true),
            Err(err) => err,
        };
        let lower = output.to_lowercase();
        if lower.contains("could not resolve to a repository")
            || lower.contains("not found")
            || lower.contains("http 404")
        {
            Ok(false)
        } else if output.is_empty() {
            Err(format!("gh repo view {repo_slug} failed."))
        } else {
            Err(format!("gh repo view {repo_slug} failed: {output}"))
        }
    }
}
