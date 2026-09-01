use super::RepositorySnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryRemoteAction {
    Publish,
    Pull,
    Push,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryRemoteSuggestion {
    pub action: RepositoryRemoteAction,
    pub title: String,
    pub detail: String,
    pub button_label: String,
}

pub fn repository_remote_suggestion(
    snapshot: &RepositorySnapshot,
) -> Option<RepositoryRemoteSuggestion> {
    let remote = snapshot.remote_name.as_deref()?;
    if !snapshot.has_upstream {
        Some(RepositoryRemoteSuggestion {
            action: RepositoryRemoteAction::Publish,
            title: "Publish your branch".to_string(),
            detail: format!(
                "Publish the local branch '{}' to the remote '{}' to share your commits.",
                snapshot.branch, remote
            ),
            button_label: "Publish branch".to_string(),
        })
    } else if snapshot.behind > 0 {
        Some(RepositoryRemoteSuggestion {
            action: RepositoryRemoteAction::Pull,
            title: if snapshot.behind == 1 {
                "Pull 1 commit from remote".to_string()
            } else {
                format!("Pull {} commits from remote", snapshot.behind)
            },
            detail: format!(
                "The current branch '{}' has commits on the remote that do not exist locally.",
                snapshot.branch
            ),
            button_label: format!("Pull {remote}"),
        })
    } else if snapshot.ahead > 0 {
        Some(RepositoryRemoteSuggestion {
            action: RepositoryRemoteAction::Push,
            title: if snapshot.ahead == 1 {
                "Push 1 commit to remote".to_string()
            } else {
                format!("Push {} commits to remote", snapshot.ahead)
            },
            detail: "You have local commits that haven't been pushed to the remote.".to_string(),
            button_label: format!("Push {remote}"),
        })
    } else {
        None
    }
}
