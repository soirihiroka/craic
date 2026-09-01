#[derive(Clone, Copy)]
enum RepositoryRequestGroup {
    Core,
    History,
    Files,
    Containers,
    Media,
    CommitSettings,
    Changes,
}

async fn run_repository_service(
    mut repository_request_rx: tokio::sync::mpsc::Receiver<RepositoryRequest>,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    repository_service_cancellation: WorkspaceCancellationToken,
    retired_jobs: RetiredJobSender,
) {
    loop {
        let request = tokio::select! {
            _ = repository_service_cancellation.cancelled() => break,
            request = repository_request_rx.recv() => request,
        };
        let Some(request) = request else {
            break;
        };
        let group = match &request {
            RepositoryRequest::InitializeRepository { .. }
            | RepositoryRequest::Load { .. }
            | RepositoryRequest::LoadQuickActions { .. }
            | RepositoryRequest::Refresh { .. }
            | RepositoryRequest::RunBranchAction { .. }
            | RepositoryRequest::RunGitAction { .. }
            | RepositoryRequest::SaveQuickActionConfiguration { .. } => RepositoryRequestGroup::Core,
            RepositoryRequest::LoadFileBytesComparison { .. }
            | RepositoryRequest::LoadFileComparison { .. }
            | RepositoryRequest::LoadHistoryBytesComparison { .. }
            | RepositoryRequest::LoadHistoryCommit { .. }
            | RepositoryRequest::LoadHistoryComparison { .. }
            | RepositoryRequest::LoadHistoryPage { .. }
            | RepositoryRequest::RunHistoryAction { .. } => RepositoryRequestGroup::History,
            RepositoryRequest::AuthorizeFileSudo { .. }
            | RepositoryRequest::DownloadWorkspaceFile { .. }
            | RepositoryRequest::HighlightWorkspaceText { .. }
            | RepositoryRequest::LoadFilesTree { .. }
            | RepositoryRequest::LoadWorkspaceFile { .. }
            | RepositoryRequest::LoadWorkspaceFolder { .. }
            | RepositoryRequest::LoadWorkspaceSqlitePage { .. }
            | RepositoryRequest::LoadWorkspaceSqliteSchema { .. }
            | RepositoryRequest::RunFileMutation { .. }
            | RepositoryRequest::SaveWorkspaceFile { .. } => RepositoryRequestGroup::Files,
            RepositoryRequest::LoadContainerDetail { .. }
            | RepositoryRequest::LoadContainers { .. }
            | RepositoryRequest::RunComposeAction { .. }
            | RepositoryRequest::RunContainerAction { .. } => RepositoryRequestGroup::Containers,
            RepositoryRequest::LoadAgentImage { .. }
            | RepositoryRequest::LoadAvatar { .. }
            | RepositoryRequest::LoadCommitAuthors { .. }
            | RepositoryRequest::ResolveAgentFileLink { .. } => RepositoryRequestGroup::Media,
            RepositoryRequest::Commit { .. }
            | RepositoryRequest::GenerateCommitMessage { .. }
            | RepositoryRequest::LoadCommitMessageModels { .. }
            | RepositoryRequest::LoadCommitMessageSettings { .. }
            | RepositoryRequest::LoadWorkspaceSettings { .. }
            | RepositoryRequest::SaveCommitAuthor { .. }
            | RepositoryRequest::SaveCommitMessageModel { .. }
            | RepositoryRequest::SaveCommitMessageProvider { .. }
            | RepositoryRequest::SaveWorkspaceSettings { .. } => RepositoryRequestGroup::CommitSettings,
            RepositoryRequest::AddIgnorePattern { .. }
            | RepositoryRequest::Discard { .. }
            | RepositoryRequest::Stash { .. } => RepositoryRequestGroup::Changes,
        };
        match group {
            RepositoryRequestGroup::Core => handle_repository_core(
                request,
                repository_completion_tx.clone(),
                retired_jobs.clone(),
            )
            .await,
            RepositoryRequestGroup::History => handle_repository_history(
                request,
                repository_completion_tx.clone(),
                retired_jobs.clone(),
            )
            .await,
            RepositoryRequestGroup::Files => handle_repository_files(
                request,
                repository_completion_tx.clone(),
                retired_jobs.clone(),
            )
            .await,
            RepositoryRequestGroup::Containers => handle_repository_containers(
                request,
                repository_completion_tx.clone(),
                retired_jobs.clone(),
            )
            .await,
            RepositoryRequestGroup::Media => handle_repository_media(
                request,
                repository_completion_tx.clone(),
                retired_jobs.clone(),
            )
            .await,
            RepositoryRequestGroup::CommitSettings => handle_repository_commit_settings(
                request,
                repository_completion_tx.clone(),
                retired_jobs.clone(),
            )
            .await,
            RepositoryRequestGroup::Changes => handle_repository_changes(
                request,
                repository_completion_tx.clone(),
                retired_jobs.clone(),
            )
            .await,
        }
    }
    log::info!("native repository service stopped");
}
