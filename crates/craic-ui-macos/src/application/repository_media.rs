async fn handle_repository_media(
    request: RepositoryRequest,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    _retired_jobs: RetiredJobSender,
) {
    match request {
        RepositoryRequest::LoadAvatar {
            cache_key,
            source,
            handle,
        } => {
            let worker_key = cache_key.clone();
            let result = async {
                let cache_lookup_key = worker_key.clone();
                if let Some(bytes) = tokio::task::spawn_blocking(move || {
                    github::cached_avatar_bytes(&cache_lookup_key)
                })
                .await
                .map_err(|error| format!("Avatar cache task failed: {error}"))?
                {
                    return Ok(bytes);
                }
                let url = tokio::task::spawn_blocking(move || -> Result<String, String> {
                    let url = match source {
                        AvatarSource::Email(email) => handle.github_avatar_url_for_email(&email)?,
                        AvatarSource::Url(url) => url,
                    };
                    Ok(url)
                })
                .await
                .map_err(|error| format!("Avatar URL task failed: {error}"))??;
                let bytes = github::download_avatar_async(&url).await?;
                let cache_write_key = worker_key;
                let cached_bytes = bytes.clone();
                tokio::task::spawn_blocking(move || {
                    github::cache_avatar_bytes(&cache_write_key, &cached_bytes);
                })
                .await
                .map_err(|error| format!("Avatar cache write task failed: {error}"))?;
                Ok(bytes)
            }
            .await;
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::Avatar { cache_key, result },
                ))
                .is_err()
            {
                log::warn!("avatar completion dropped during shutdown");
            }
        }
        RepositoryRequest::LoadCommitAuthors {
            workspace_id,
            handle,
        } => {
            let result = tokio::task::spawn_blocking(move || handle.github_commit_email_options())
                .await
                .unwrap_or_else(|error| Err(format!("GitHub email loading task failed: {error}")));
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::CommitAuthors {
                        workspace_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("commit author options dropped during shutdown");
            }
        }
        RepositoryRequest::ResolveAgentFileLink {
            workspace_id,
            handle,
            path,
            line,
            column,
        } => {
            let display_path = path.clone();
            let result = tokio::task::spawn_blocking(move || {
                handle.resolve_workspace_file_link(path.strip_prefix("file://").unwrap_or(&path))
            })
            .await
            .unwrap_or_else(|error| Err(format!("File-link resolution task failed: {error}")));
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::AgentFileLinkResolved {
                        workspace_id,
                        line,
                        column,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!(
                    "Codex file-link completion dropped during shutdown path={display_path}"
                );
            }
        }
        _ => unreachable!("repository request routed to the wrong handler"),
    }
}
