async fn handle_repository_history(
    request: RepositoryRequest,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    retired_jobs: RetiredJobSender,
) {
    match request {
        RepositoryRequest::LoadFileComparison {
            workspace_id,
            handle,
            path,
            request_id,
            cancellation,
        } => {
            let load_handle = handle.clone();
            let load_path = path.clone();
            let result = match wait_workspace_future(
                &cancellation,
                &retired_jobs,
                "file-comparison",
                async move { load_handle.comparison_async(&load_path).await },
            )
            .await
            {
                NativeJobWait::Completed(Ok(result)) => result,
                NativeJobWait::Completed(Err(error)) => {
                    Err(format!("File comparison task failed: {error}"))
                }
                NativeJobWait::WorkspaceChanged => {
                    log::debug!("file comparison canceled workspace={workspace_id} path={path}");
                    return;
                }
                NativeJobWait::TimedOut => {
                    Err("File comparison timed out.".to_string())
                }
            };
            let result = match result {
                Ok(comparison) => {
                    let syntax_path = path.clone();
                    let task = tokio::task::spawn_blocking(move || {
                        prepare_diff(&syntax_path, comparison)
                    });
                    let Some(result) =
                        until_workspace_change(
                            &cancellation,
                            &retired_jobs,
                            "file-diff-preparation",
                            task,
                        )
                        .await
                    else {
                        log::debug!(
                            "diff preparation canceled workspace={workspace_id} path={path}"
                        );
                        return;
                    };
                    result.map_err(|error| {
                        format!("Diff syntax worker did not complete: {error}")
                    })
                }
                Err(error) => Err(error),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::FileComparison {
                        workspace_id,
                        path,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("file comparison dropped during shutdown");
            }
        }
        RepositoryRequest::LoadFileBytesComparison {
            workspace_id,
            handle,
            path,
            request_id,
            cancellation,
        } => {
            let load_handle = handle.clone();
            let load_path = path.clone();
            let result = match wait_workspace_future(
                &cancellation,
                &retired_jobs,
                "file-bytes-comparison",
                async move { load_handle.bytes_comparison_async(&load_path).await },
            )
            .await
            {
                NativeJobWait::Completed(Ok(result)) => result,
                NativeJobWait::Completed(Err(error)) => {
                    Err(format!("Image comparison task failed: {error}"))
                }
                NativeJobWait::WorkspaceChanged => {
                    log::debug!("image comparison canceled workspace={workspace_id} path={path}");
                    return;
                }
                NativeJobWait::TimedOut => {
                    Err("Image comparison timed out.".to_string())
                }
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::FileBytesComparison {
                        workspace_id,
                        path,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("image comparison dropped during shutdown");
            }
        }
        RepositoryRequest::LoadHistoryPage {
            workspace_id,
            handle,
            query,
            after,
            generation,
            cancellation,
        } => {
            let result = match wait_workspace_future(
                &cancellation,
                &retired_jobs,
                "history-page",
                async move {
                if query.is_empty() {
                    handle.commit_page_async(after.as_deref(), 32).await
                } else {
                    handle
                        .commit_search_page_async(&query, after.as_deref(), 32)
                        .await
                }
                },
            )
            .await
            {
                NativeJobWait::Completed(Ok(result)) => result,
                NativeJobWait::Completed(Err(error)) => {
                    Err(format!("History page task failed: {error}"))
                }
                NativeJobWait::WorkspaceChanged => {
                    log::debug!("history page canceled workspace={workspace_id}");
                    return;
                }
                NativeJobWait::TimedOut => Err("History page timed out.".to_string()),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::HistoryPage {
                        workspace_id,
                        generation,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("history page dropped during shutdown");
            }
        }
        RepositoryRequest::LoadHistoryCommit {
            workspace_id,
            handle,
            hash,
            request_id,
            cancellation,
        } => {
            let detail_handle = handle.clone();
            let detail_hash = hash.clone();
            let detail = match wait_workspace_future(
                &cancellation,
                &retired_jobs,
                "history-commit-detail",
                async move { detail_handle.commit_details_async(&detail_hash).await },
            )
            .await
            {
                NativeJobWait::Completed(Ok(result)) => result,
                NativeJobWait::Completed(Err(error)) => {
                    Err(format!("Commit details task failed: {error}"))
                }
                NativeJobWait::WorkspaceChanged => {
                    log::debug!(
                        "history commit canceled workspace={workspace_id} hash={hash}"
                    );
                    return;
                }
                NativeJobWait::TimedOut => {
                    Err("Commit details timed out.".to_string())
                }
            };
            let result = match detail {
                Ok(commit) => {
                    let files_handle = handle.clone();
                    let files_hash = hash.clone();
                    match wait_workspace_future(
                        &cancellation,
                        &retired_jobs,
                        "history-changed-files",
                        async move {
                            files_handle.commit_changed_files_async(&files_hash).await
                        },
                    )
                    .await
                    {
                        NativeJobWait::Completed(Ok(Ok(files))) => {
                            let parent_handle = handle.clone();
                            let parent_hash = hash.clone();
                            match wait_workspace_future(
                                &cancellation,
                                &retired_jobs,
                                "history-parent-hash",
                                async move {
                                    parent_handle
                                        .commit_parent_hash_async(&parent_hash)
                                        .await
                                },
                            )
                            .await
                            {
                                NativeJobWait::Completed(Ok(Ok(parent_hash))) => {
                                    Ok((commit, files, parent_hash, true))
                                }
                                NativeJobWait::Completed(Ok(Err(error))) => {
                                    log::warn!(
                                        "history parent load failed workspace={workspace_id} hash={hash} error={error}"
                                    );
                                    Ok((commit, files, None, false))
                                }
                                NativeJobWait::Completed(Err(error)) => {
                                    log::warn!(
                                        "history parent task failed workspace={workspace_id} hash={hash} error={error}"
                                    );
                                    Ok((commit, files, None, false))
                                }
                                NativeJobWait::WorkspaceChanged => {
                                    log::debug!(
                                        "history parent load canceled workspace={workspace_id} hash={hash}"
                                    );
                                    return;
                                }
                                NativeJobWait::TimedOut => {
                                    log::warn!(
                                        "history parent load timed out workspace={workspace_id} hash={hash}"
                                    );
                                    Ok((commit, files, None, false))
                                }
                            }
                        }
                        NativeJobWait::Completed(Ok(Err(error))) => Err(error),
                        NativeJobWait::Completed(Err(error)) => {
                            Err(format!("Changed files task failed: {error}"))
                        }
                        NativeJobWait::WorkspaceChanged => {
                            log::debug!(
                                "history changed-files load canceled workspace={workspace_id} hash={hash}"
                            );
                            return;
                        }
                        NativeJobWait::TimedOut => {
                            Err("Changed files timed out.".to_string())
                        }
                    }
                }
                Err(error) => Err(error),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::HistoryCommit {
                        workspace_id,
                        hash,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("history commit dropped during shutdown");
            }
        }
        RepositoryRequest::RunHistoryAction {
            workspace_id,
            handle,
            action,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "history action discarded before start workspace={workspace_id}"
                );
                return;
            }
            let (progress, failure_title, fallback) = match &action {
                HistoryAction::Checkout { parent, .. } => (
                    if *parent {
                        "Checking out parent commit…"
                    } else {
                        "Checking out commit…"
                    },
                    "Checkout Failed",
                    if *parent {
                        "Checked out parent commit."
                    } else {
                        "Checked out commit."
                    },
                ),
                HistoryAction::CreateBranch { .. } => (
                    "Creating branch…",
                    "Create Branch Failed",
                    "Created and checked out branch.",
                ),
                HistoryAction::CreateTag { .. } => (
                    "Creating tag…",
                    "Create Tag Failed",
                    "Created tag.",
                ),
                HistoryAction::CherryPick(_) => (
                    "Cherry-picking commit…",
                    "Cherry-Pick Failed",
                    "Cherry-picked commit.",
                ),
                HistoryAction::Revert(_) => (
                    "Reverting commit…",
                    "Revert Failed",
                    "Reverted commit.",
                ),
                HistoryAction::Amend { .. } => (
                    "Amending HEAD…",
                    "Amend Failed",
                    "Amended HEAD.",
                ),
                HistoryAction::Reset { mode, .. } => (
                    "Resetting current branch…",
                    "Reset Failed",
                    if *mode == ResetMode::Hard {
                        "Hard reset current branch."
                    } else {
                        "Reset current branch."
                    },
                ),
            };
            let _ = repository_completion_tx.send(FrontendCompletion::Repository(
                RepositoryCompletion::HistoryActionProgress {
                    workspace_id: workspace_id.clone(),
                    cancellation: cancellation.clone(),
                    message: progress.to_string(),
                },
            ));
            let operation_handle = handle.clone();
            // Git mutations cannot be safely aborted once the blocking operation has
            // started. Keep the repository actor serialized until it finishes instead
            // of timing out and allowing a conflicting mutation to begin.
            let result = match action {
                HistoryAction::Checkout { hash, .. } => {
                    operation_handle.checkout_commit_async(&hash).await
                }
                HistoryAction::CreateBranch { branch, hash } => {
                    operation_handle
                        .create_branch_at_commit_async(&branch, &hash)
                        .await
                }
                HistoryAction::CreateTag { tag, hash } => {
                    operation_handle.create_tag_async(&tag, &hash).await
                }
                HistoryAction::CherryPick(hash) => {
                    operation_handle.cherry_pick_commit_async(&hash).await
                }
                HistoryAction::Revert(hash) => {
                    operation_handle.revert_commit_async(&hash).await
                }
                HistoryAction::Amend {
                    summary,
                    description,
                } => {
                    operation_handle
                        .amend_head_async(&summary, &description)
                        .await
                }
                HistoryAction::Reset { hash, mode } => {
                    operation_handle.reset_to_commit_async(&hash, mode).await
                }
            };
            if cancellation.is_cancelled() {
                log::debug!(
                    "history action completed after workspace cancellation workspace={workspace_id}"
                );
                return;
            }
            let message = match result {
                Ok(message) if !message.trim().is_empty() => message,
                Ok(_) => fallback.to_string(),
                Err(message) => {
                    let _ = repository_completion_tx.send(
                        FrontendCompletion::Repository(
                            RepositoryCompletion::HistoryActionFailed {
                                workspace_id: workspace_id.clone(),
                                cancellation: cancellation.clone(),
                                title: failure_title,
                                message,
                            },
                        ),
                    );
                    return;
                }
            };
            let refresh_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                refresh_handle.load_workspace_snapshot()
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "history-action-refresh",
                task,
            )
            .await
            else {
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::HistoryActionFinished {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle,
                        result,
                        message,
                    },
                ))
                .is_err()
            {
                log::warn!("history action completion dropped during shutdown");
            }
        }
        RepositoryRequest::LoadHistoryComparison {
            workspace_id,
            handle,
            hash,
            path,
            request_id,
            cancellation,
        } => {
            let load_handle = handle.clone();
            let load_hash = hash.clone();
            let load_path = path.clone();
            let result = match wait_workspace_future(
                &cancellation,
                &retired_jobs,
                "history-comparison",
                async move {
                    load_handle
                        .commit_comparison_async(&load_hash, &load_path)
                        .await
                },
            )
            .await
            {
                NativeJobWait::Completed(Ok(result)) => result,
                NativeJobWait::Completed(Err(error)) => {
                    Err(format!("History comparison task failed: {error}"))
                }
                NativeJobWait::WorkspaceChanged => {
                    log::debug!(
                        "history comparison canceled workspace={workspace_id} hash={hash} path={path}"
                    );
                    return;
                }
                NativeJobWait::TimedOut => {
                    Err("History comparison timed out.".to_string())
                }
            };
            let result = match result {
                Ok(comparison) => {
                    let syntax_path = path.clone();
                    let task = tokio::task::spawn_blocking(move || {
                        prepare_diff(&syntax_path, comparison)
                    });
                    let Some(result) =
                        until_workspace_change(
                            &cancellation,
                            &retired_jobs,
                            "history-diff-preparation",
                            task,
                        )
                        .await
                    else {
                        log::debug!(
                            "history diff preparation canceled workspace={workspace_id} hash={hash} path={path}"
                        );
                        return;
                    };
                    result.map_err(|error| {
                        format!("Diff syntax worker did not complete: {error}")
                    })
                }
                Err(error) => Err(error),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::HistoryComparison {
                        workspace_id,
                        hash,
                        path,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("history comparison dropped during shutdown");
            }
        }
        RepositoryRequest::LoadHistoryBytesComparison {
            workspace_id,
            handle,
            hash,
            path,
            request_id,
            cancellation,
        } => {
            let load_hash = hash.clone();
            let load_path = path.clone();
            let result = match wait_workspace_future(
                &cancellation,
                &retired_jobs,
                "history-bytes-comparison",
                async move {
                    handle
                        .commit_bytes_comparison_async(&load_hash, &load_path)
                        .await
                },
            )
            .await
            {
                NativeJobWait::Completed(Ok(result)) => result,
                NativeJobWait::Completed(Err(error)) => {
                    Err(format!("History binary comparison task failed: {error}"))
                }
                NativeJobWait::WorkspaceChanged => {
                    log::debug!(
                        "history binary comparison canceled workspace={workspace_id} hash={hash} path={path}"
                    );
                    return;
                }
                NativeJobWait::TimedOut => {
                    Err("History binary comparison timed out.".to_string())
                }
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::HistoryBytesComparison {
                        workspace_id,
                        hash,
                        path,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("history binary comparison dropped during shutdown");
            }
        }
        _ => unreachable!("repository request routed to the wrong handler"),
    }
}
