async fn handle_repository_changes(
    request: RepositoryRequest,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    retired_jobs: RetiredJobSender,
) {
    match request {
        RepositoryRequest::Discard {
            workspace_id,
            handle,
            paths,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "native discard discarded before start workspace={workspace_id}"
                );
                return;
            }
            log::info!(
                "native discard started workspace={} files={}",
                workspace_id,
                paths.len()
            );
            let mut failure = None;
            let mut canceled = false;
            for path in paths {
                let discard_handle = handle.clone();
                let result = discard_handle.discard_path_async(&path).await;
                if cancellation.is_cancelled() {
                    log::debug!(
                        "native discard completed after workspace cancellation workspace={workspace_id} path={path}"
                    );
                    canceled = true;
                    break;
                }
                match result {
                    Ok(_) => {}
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            if canceled {
                return;
            }
            if let Some(message) = failure {
                let _ = repository_completion_tx.send(
                    FrontendCompletion::Repository(
                        RepositoryCompletion::ChangesFailed {
                            cancellation: cancellation.clone(),
                            title: "Discard Failed",
                            message,
                        },
                    ),
                );
                return;
            }
            let reload_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                reload_handle.load_workspace_snapshot()
            });
            let Some(snapshot) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "discard-refresh",
                task,
            )
            .await
            else {
                log::debug!("discard refresh canceled workspace={workspace_id}");
                return;
            };
            let snapshot = snapshot.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::Snapshot {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle: Some(handle),
                        core_request: None,
                        result: snapshot,
                    },
                ))
                .is_err()
            {
                log::warn!("discard completion dropped during shutdown");
            }
        }
        RepositoryRequest::Stash {
            workspace_id,
            handle,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "native stash discarded before start workspace={workspace_id}"
                );
                return;
            }
            log::info!("native stash started workspace={workspace_id}");
            let stash_handle = handle.clone();
            let result = stash_handle.stash_changes_async().await;
            if cancellation.is_cancelled() {
                log::debug!(
                    "native stash completed after workspace cancellation workspace={workspace_id}"
                );
                return;
            }
            match result {
                Ok(message) => {
                    log::info!(
                        "native stash finished workspace={} message={}",
                        workspace_id,
                        message.trim()
                    );
                }
                Err(message) => {
                    let _ = repository_completion_tx.send(
                        FrontendCompletion::Repository(
                            RepositoryCompletion::ChangesFailed {
                                cancellation: cancellation.clone(),
                                title: "Stash Failed",
                                message,
                            },
                        ),
                    );
                    return;
                }
            }
            let reload_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                reload_handle.load_workspace_snapshot()
            });
            let Some(snapshot) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "stash-refresh",
                task,
            )
            .await
            else {
                log::debug!("stash refresh canceled workspace={workspace_id}");
                return;
            };
            let snapshot = snapshot.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::Snapshot {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle: Some(handle),
                        core_request: None,
                        result: snapshot,
                    },
                ))
                .is_err()
            {
                log::warn!("stash completion dropped during shutdown");
            }
        }
        RepositoryRequest::AddIgnorePattern {
            workspace_id,
            handle,
            pattern,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "native ignore-pattern discarded before start workspace={workspace_id}"
                );
                return;
            }
            log::info!(
                "native ignore-pattern started workspace={} pattern={}",
                workspace_id,
                pattern
            );
            let ignore_handle = handle.clone();
            let result = ignore_handle.add_ignore_pattern_async(pattern).await;
            if cancellation.is_cancelled() {
                log::debug!(
                    "native ignore-pattern completed after workspace cancellation workspace={workspace_id}"
                );
                return;
            }
            match result {
                Ok(message) => {
                    log::info!(
                        "native ignore-pattern finished workspace={} message={}",
                        workspace_id,
                        message.trim()
                    );
                }
                Err(message) => {
                    let _ = repository_completion_tx.send(
                        FrontendCompletion::Repository(
                            RepositoryCompletion::ChangesFailed {
                                cancellation: cancellation.clone(),
                                title: "Ignore Failed",
                                message,
                            },
                        ),
                    );
                    return;
                }
            }
            let reload_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                reload_handle.load_workspace_snapshot()
            });
            let Some(snapshot) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "ignore-refresh",
                task,
            )
            .await
            else {
                log::debug!("ignore refresh canceled workspace={workspace_id}");
                return;
            };
            let snapshot = snapshot.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::Snapshot {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle: Some(handle),
                        core_request: None,
                        result: snapshot,
                    },
                ))
                .is_err()
            {
                log::warn!("ignore completion dropped during shutdown");
            }
        }
        _ => unreachable!("repository request routed to the wrong handler"),
    }
}
