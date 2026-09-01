async fn handle_repository_media(
    request: RepositoryRequest,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    retired_jobs: RetiredJobSender,
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
                let url = tokio::task::spawn_blocking(
                    move || -> Result<String, String> {
                    let url = match source {
                        AvatarSource::Email(email) => {
                            handle.github_avatar_url_for_email(&email)?
                        }
                        AvatarSource::Url(url) => url,
                    };
                        Ok(url)
                    },
                )
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
            let result = tokio::task::spawn_blocking(move || {
                handle.github_commit_email_options()
            })
                .await
                .unwrap_or_else(|error| {
                    Err(format!("GitHub email loading task failed: {error}"))
                });
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
                handle.resolve_workspace_file_link(
                    path.strip_prefix("file://").unwrap_or(&path),
                )
            })
            .await
            .unwrap_or_else(|error| {
                Err(format!("File-link resolution task failed: {error}"))
            });
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
        RepositoryRequest::LoadAgentImage {
            workspace_id,
            generation,
            item_id,
            source,
            access,
            cancellation,
        } => {
            let completion_source = source.clone();
            let result = match source {
                NativeAgentTranscriptImageSource::DataUri(data_uri) => {
                    let task = tokio::task::spawn_blocking(move || {
                        decode_agent_image_data_uri(&data_uri)
                    });
                    let Some(result) =
                        until_workspace_change(
                            &cancellation,
                            &retired_jobs,
                            "agent-image-decode",
                            task,
                        )
                        .await
                    else {
                        log::debug!(
                            "Codex inline image decode canceled workspace={workspace_id} item_id={item_id}"
                        );
                        return;
                    };
                    result.unwrap_or_else(|error| {
                        Err(format!("Image decode task failed: {error}"))
                    })
                }
                NativeAgentTranscriptImageSource::WorkspacePath(path) => {
                    let node = agent_image_node_path(access.as_ref(), &path);
                    match node {
                        Ok(node) => {
                            let cancel_requested = Arc::new(AtomicBool::new(false));
                            let events = access.read_with_info_events(FileReadRequest {
                                path: node,
                                max_bytes: Some(AGENT_IMAGE_PREVIEW_LIMIT),
                                cancel_requested: Some(cancel_requested.clone()),
                            });
                            let Some(result) = wait_file_operation(
                                events,
                                &cancellation,
                                cancel_requested,
                            )
                            .await
                            else {
                                log::debug!(
                                    "Codex workspace image read canceled workspace={workspace_id} item_id={item_id} path={path}"
                                );
                                return;
                            };
                            result.and_then(FileRead::into_bytes)
                        }
                        Err(error) => Err(error),
                    }
                }
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::AgentImage {
                        workspace_id,
                        generation,
                        item_id,
                        source: completion_source,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("Codex transcript image completion dropped during shutdown");
            }
        }
        _ => unreachable!("repository request routed to the wrong handler"),
    }
}
