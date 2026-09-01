async fn handle_repository_core(
    request: RepositoryRequest,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    retired_jobs: RetiredJobSender,
) {
    match request {
        RepositoryRequest::Load {
            workspace,
            cancellation,
        } => {
            let workspace_id = workspace.selection_id();
            let task = tokio::task::spawn_blocking(move || {
                load_workspace_snapshot(&workspace)
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "repository-load",
                task,
            )
            .await
            else {
                log::debug!("repository load canceled workspace={workspace_id}");
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Repository load task failed: {error}"))
            });
            let (handle, result) = match result {
                Ok((handle, snapshot)) => (Some(handle), Ok(snapshot)),
                Err(error) => (None, Err(error)),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::Snapshot {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle,
                        core_request: None,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("repository result dropped during shutdown");
            }
        }
        RepositoryRequest::Refresh {
            workspace_id,
            handle,
            core_request,
            cancellation,
        } => {
            let refresh_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                refresh_handle.load_workspace_snapshot()
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "repository-refresh",
                task,
            )
            .await
            else {
                log::debug!("repository refresh canceled workspace={workspace_id}");
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::Snapshot {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle: Some(handle),
                        core_request,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("repository refresh dropped during shutdown");
            }
        }
        RepositoryRequest::InitializeRepository {
            workspace_id,
            handle,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "repository initialization discarded before start workspace={workspace_id}"
                );
                return;
            }
            let result = handle.initialize_repository_async().await;
            if cancellation.is_cancelled() {
                log::debug!(
                    "repository initialization completed after workspace cancellation workspace={workspace_id}"
                );
                return;
            }
            if let Err(message) = result {
                let _ = repository_completion_tx.send(
                    FrontendCompletion::Repository(
                        RepositoryCompletion::RepositoryInitializationFailed {
                            workspace_id,
                            cancellation: cancellation.clone(),
                            message,
                        },
                    ),
                );
                return;
            }
            let refresh_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                refresh_handle.load_workspace_snapshot()
            });
            let result = task.await.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if cancellation.is_cancelled() {
                return;
            }
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::RepositoryInitializationFinished {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!(
                    "repository initialization completion dropped during shutdown"
                );
            }
        }
        RepositoryRequest::LoadQuickActions {
            workspace_id,
            repo_path,
            generation,
            cancellation,
        } => {
            let task = tokio::task::spawn_blocking(move || {
                if !repo_path.is_dir() {
                    return Err(format!(
                        "The workspace directory does not exist: {}",
                        repo_path.display()
                    ));
                }
                let mut targets = quick_action::discover(&repo_path);
                targets.extend(quick_action::discover_additional_quick_actions(
                    &workspace_config::quick_action_additional_commands(&repo_path),
                ));
                let configs = workspace_config::quick_action_config(&repo_path)
                    .unwrap_or_else(|| vec![QuickActionConfig::default()]);
                Ok(NativeQuickActions {
                    targets,
                    configs,
                })
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "quick-action-discovery",
                task,
            )
            .await
            else {
                log::debug!(
                    "quick action discovery canceled workspace={workspace_id} generation={generation}"
                );
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Quick Action discovery task failed: {error}"))
            });
            let _ = repository_completion_tx.send(FrontendCompletion::Repository(
                RepositoryCompletion::QuickActions {
                    workspace_id,
                    generation,
                    result,
                },
            ));
        }
        RepositoryRequest::SaveQuickActionConfiguration {
            workspace_id,
            repo_path,
            configs,
        } => {
            let result = tokio::task::spawn_blocking(move || {
                workspace_config::save_quick_action_config(&repo_path, configs)
            })
            .await
            .unwrap_or_else(|error| {
                Err(format!("Quick Action save task failed: {error}"))
            });
            let _ = repository_completion_tx.send(FrontendCompletion::Repository(
                RepositoryCompletion::QuickActionConfigurationSaved {
                    workspace_id,
                    result,
                },
            ));
        }
        RepositoryRequest::RunGitAction {
            workspace_id,
            handle,
            snapshot,
            action,
            stash_before,
            cancellation,
        } => {
            let mut snapshot = snapshot;
            if cancellation.is_cancelled() {
                log::debug!(
                    "native git action discarded before start workspace={workspace_id}"
                );
                return;
            }
            if stash_before {
                let _ = repository_completion_tx.send(
                    FrontendCompletion::Repository(
                        RepositoryCompletion::ActionProgress {
                            workspace_id: workspace_id.clone(),
                            cancellation: cancellation.clone(),
                            message: "Stashing changes…".to_string(),
                        },
                    ),
                );
                let stash_handle = handle.clone();
                let stash_result = stash_handle.stash_changes_async().await;
                if cancellation.is_cancelled() {
                    log::debug!(
                        "stash before git action completed after workspace cancellation workspace={workspace_id}"
                    );
                    return;
                }
                match stash_result {
                    Ok(message) => log::info!(
                        "native stash before retry finished workspace={} message={}",
                        workspace_id,
                        message.trim()
                    ),
                    Err(message) => {
                        let _ = repository_completion_tx.send(
                            FrontendCompletion::Repository(
                                RepositoryCompletion::ActionFailed {
                                    workspace_id: workspace_id.clone(),
                                    cancellation: cancellation.clone(),
                                    title: "Stash Failed",
                                    message,
                                },
                            ),
                        );
                        return;
                    }
                }
            }

            if matches!(action, NativeRemoteAction::Contextual)
                && !stash_before
            {
                let _ = repository_completion_tx.send(
                    FrontendCompletion::Repository(
                        RepositoryCompletion::ActionProgress {
                            workspace_id: workspace_id.clone(),
                            cancellation: cancellation.clone(),
                            message: "Checking repository status…".to_string(),
                        },
                    ),
                );
                let refresh_handle = handle.clone();
                let refresh_task = tokio::task::spawn_blocking(move || {
                    refresh_handle.load_workspace_snapshot()
                });
                let Some(refreshed) =
                    until_workspace_change(
                        &cancellation,
                        &retired_jobs,
                        "git-action-refresh",
                        refresh_task,
                    )
                    .await
                else {
                    log::debug!(
                        "native contextual sync canceled before initial snapshot workspace={workspace_id}"
                    );
                    return;
                };
                let refreshed = refreshed.unwrap_or_else(|error| {
                    Err(format!("Repository refresh task failed: {error}"))
                });
                snapshot = match refreshed {
                    Ok(WorkspaceSnapshot::Repository(refreshed)) => refreshed,
                    Ok(WorkspaceSnapshot::NonRepository { .. }) => {
                        let _ = repository_completion_tx.send(
                            FrontendCompletion::Repository(
                                RepositoryCompletion::ActionFailed {
                                    workspace_id: workspace_id.clone(),
                                    cancellation: cancellation.clone(),
                                    title: "Repository Error",
                                    message: "The workspace is no longer a Git repository."
                                        .to_string(),
                                },
                            ),
                        );
                        return;
                    }
                    Err(message) => {
                        let _ = repository_completion_tx.send(
                            FrontendCompletion::Repository(
                                RepositoryCompletion::ActionFailed {
                                    workspace_id: workspace_id.clone(),
                                    cancellation: cancellation.clone(),
                                    title: "Repository Error",
                                    message,
                                },
                            ),
                        );
                        return;
                    }
                };
            }

            if matches!(action, NativeRemoteAction::Contextual)
                && snapshot.has_upstream
                && !stash_before
            {
                let remote_name = snapshot
                    .remote_name
                    .clone()
                    .unwrap_or_else(|| "remote".to_string());
                let _ = repository_completion_tx.send(
                    FrontendCompletion::Repository(
                        RepositoryCompletion::ActionProgress {
                            workspace_id: workspace_id.clone(),
                            cancellation: cancellation.clone(),
                            message: format!("Fetching {remote_name}…"),
                        },
                    ),
                );
                log::info!(
                    "native contextual sync fetching before action workspace={} remote={}",
                    workspace_id,
                    remote_name
                );

                let mut fetch_events = handle.fetch_with_progress(None);
                let fetch_completion_tx = repository_completion_tx.clone();
                let fetch_workspace_id = workspace_id.clone();
                let fetch_cancellation = cancellation.clone();
                let task = tokio::spawn(async move {
                    loop {
                        let Some(event) = fetch_events.recv().await else {
                            break Err(
                                "Fetch ended before reporting completion.".to_string()
                            );
                        };
                        match event {
                            GitCommandEvent::Progress { message } => {
                                if !fetch_cancellation.is_cancelled() {
                                    let _ = fetch_completion_tx.send(
                                        FrontendCompletion::Repository(
                                            RepositoryCompletion::ActionProgress {
                                                workspace_id: fetch_workspace_id.clone(),
                                                cancellation: fetch_cancellation.clone(),
                                                message,
                                            },
                                        ),
                                    );
                                }
                            }
                            GitCommandEvent::Completed { message } => {
                                break Ok(message
                                    .map(|message| message.trim().to_string())
                                    .filter(|message| !message.is_empty()));
                            }
                            GitCommandEvent::Failed { message } => break Err(message),
                        }
                    }
                });
                let fetch_result = task.await;
                if cancellation.is_cancelled() {
                    log::debug!(
                        "native contextual fetch completed after workspace cancellation workspace={workspace_id}"
                    );
                    return;
                }
                let fetch_result = fetch_result.unwrap_or_else(|error| {
                    Err(format!("Fetch event task failed: {error}"))
                });
                if cancellation.is_cancelled() {
                    log::debug!(
                        "native contextual sync canceled after fetch workspace={workspace_id}"
                    );
                    return;
                }
                let fetch_completion_message = match fetch_result {
                    Ok(message) => message,
                    Err(message) => {
                        let _ = repository_completion_tx.send(
                            FrontendCompletion::Repository(
                                RepositoryCompletion::ActionFailed {
                                    workspace_id: workspace_id.clone(),
                                    cancellation: cancellation.clone(),
                                    title: "Git Operation Failed",
                                    message,
                                },
                            ),
                        );
                        return;
                    }
                };

                let _ = repository_completion_tx.send(
                    FrontendCompletion::Repository(
                        RepositoryCompletion::ActionProgress {
                            workspace_id: workspace_id.clone(),
                            cancellation: cancellation.clone(),
                            message: "Checking remote status…".to_string(),
                        },
                    ),
                );
                let reload_handle = handle.clone();
                let reload_task = tokio::task::spawn_blocking(move || {
                    reload_handle.load_workspace_snapshot()
                });
                let Some(refreshed) =
                    until_workspace_change(
                        &cancellation,
                        &retired_jobs,
                        "branch-action-refresh",
                        reload_task,
                    )
                    .await
                else {
                    log::debug!(
                        "native contextual sync canceled before post-fetch snapshot workspace={workspace_id}"
                    );
                    return;
                };
                let refreshed = refreshed.unwrap_or_else(|error| {
                    Err(format!("Repository refresh task failed: {error}"))
                });
                let refreshed = match refreshed {
                    Ok(WorkspaceSnapshot::Repository(refreshed)) => refreshed,
                    Ok(WorkspaceSnapshot::NonRepository { .. }) => {
                        let _ = repository_completion_tx.send(
                            FrontendCompletion::Repository(
                                RepositoryCompletion::ActionFailed {
                                    workspace_id: workspace_id.clone(),
                                    cancellation: cancellation.clone(),
                                    title: "Repository Error",
                                    message: "The workspace is no longer a Git repository."
                                        .to_string(),
                                },
                            ),
                        );
                        return;
                    }
                    Err(message) => {
                        let _ = repository_completion_tx.send(
                            FrontendCompletion::Repository(
                                RepositoryCompletion::ActionFailed {
                                    workspace_id: workspace_id.clone(),
                                    cancellation: cancellation.clone(),
                                    title: "Repository Error",
                                    message,
                                },
                            ),
                        );
                        return;
                    }
                };
                log::info!(
                    "native contextual sync selected post-fetch state workspace={} ahead={} behind={}",
                    workspace_id,
                    refreshed.ahead,
                    refreshed.behind
                );
                snapshot = refreshed;

                if snapshot.ahead == 0 && snapshot.behind == 0 {
                    let result = Ok(WorkspaceSnapshot::Repository(snapshot));
                    if repository_completion_tx
                        .send(FrontendCompletion::Repository(
                            RepositoryCompletion::ActionFinished {
                                workspace_id,
                                cancellation: cancellation.clone(),
                                handle,
                                result,
                                message: fetch_completion_message.or_else(|| {
                                    Some(format!("Fetched {remote_name}."))
                                }),
                            },
                        ))
                        .is_err()
                    {
                        log::warn!("git completion dropped during shutdown");
                    }
                    return;
                }
            }

            if cancellation.is_cancelled() {
                log::debug!(
                    "native git action discarded before mutation workspace={workspace_id}"
                );
                return;
            }

            let _ = repository_completion_tx.send(FrontendCompletion::Repository(
                RepositoryCompletion::ActionProgress {
                    workspace_id: workspace_id.clone(),
                    cancellation: cancellation.clone(),
                    message: "Working…".to_string(),
                },
            ));

            let events = match git_action_events(&handle, &snapshot, action) {
                Ok(events) => events,
                Err(message) => {
                    let _ = repository_completion_tx.send(
                        FrontendCompletion::Repository(
                            RepositoryCompletion::ActionFailed {
                                workspace_id: workspace_id.clone(),
                                cancellation: cancellation.clone(),
                                title: "Git Operation Failed",
                                message,
                            },
                        ),
                    );
                    return;
                }
            };
            let progress_completion_tx = repository_completion_tx.clone();
            let progress_workspace_id = workspace_id.clone();
            let progress_cancellation = cancellation.clone();
            let task = tokio::spawn(async move {
                let mut events = events;
                while let Some(event) = events.recv().await {
                    match event {
                        GitCommandEvent::Progress { message } => {
                            if !progress_cancellation.is_cancelled() {
                                let _ = progress_completion_tx.send(
                                    FrontendCompletion::Repository(
                                        RepositoryCompletion::ActionProgress {
                                            workspace_id: progress_workspace_id.clone(),
                                            cancellation: progress_cancellation.clone(),
                                            message,
                                        },
                                    ),
                                );
                            }
                        }
                        GitCommandEvent::Completed { message } => {
                            return Ok(message
                                .map(|message| message.trim().to_string())
                                .filter(|message| !message.is_empty()));
                        }
                        GitCommandEvent::Failed { message } => return Err(message),
                    }
                }
                Err("Git operation ended before reporting completion.".to_string())
            });
            let completion_message = task.await;
            if cancellation.is_cancelled() {
                log::debug!(
                    "git action completed after workspace cancellation workspace={workspace_id}"
                );
                return;
            }
            let completion_message = completion_message.unwrap_or_else(|error| {
                Err(format!("Git event task failed: {error}"))
            });
            let completion_message = match completion_message {
                Ok(message) => message,
                Err(message) => {
                    let completion = if !stash_before
                        && native_remote_action_pulls(action, &snapshot)
                        && craic_vcs::git::is_local_changes_overwritten_error(&message)
                    {
                        let files = craic_vcs::git::parse_files_to_be_overwritten(&message);
                        log::info!(
                            "native pull blocked by local changes workspace={} overwritten_files={}",
                            workspace_id,
                            files.len()
                        );
                        RepositoryCompletion::ActionNeedsStash {
                            workspace_id: workspace_id.clone(),
                            cancellation: cancellation.clone(),
                            handle: handle.clone(),
                            snapshot: snapshot.clone(),
                            action,
                            files,
                        }
                    } else {
                        RepositoryCompletion::ActionFailed {
                            workspace_id: workspace_id.clone(),
                            cancellation: cancellation.clone(),
                            title: "Git Operation Failed",
                            message,
                        }
                    };
                    let _ = repository_completion_tx
                        .send(FrontendCompletion::Repository(completion));
                    return;
                }
            };
            if let Some(message) = completion_message.as_deref() {
                log::info!(
                    "native git action completed workspace={} message={}",
                    workspace_id,
                    message
                );
            }
            let reload_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                reload_handle.load_workspace_snapshot()
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "git-action-final-refresh",
                task,
            )
            .await
            else {
                log::debug!(
                    "git action final refresh canceled workspace={workspace_id}"
                );
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::ActionFinished {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle,
                        result,
                        message: completion_message,
                    },
                ))
                .is_err()
            {
                log::warn!("git completion dropped during shutdown");
            }
        }
        RepositoryRequest::RunBranchAction {
            workspace_id,
            handle,
            action,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "branch operation discarded before start workspace={workspace_id}"
                );
                return;
            }
            let (progress_label, operation_name, empty_success_message) = match &action {
                BranchAction::Checkout(branch) => (
                    format!("Switching to {branch}…"),
                    "checkout",
                    format!("Checked out {branch}."),
                ),
                BranchAction::Create(branch) => (
                    format!("Creating {branch}…"),
                    "create",
                    format!("Created and checked out {branch}."),
                ),
                BranchAction::Merge(branch) => (
                    format!("Merging {branch}…"),
                    "merge",
                    "Merge completed.".to_string(),
                ),
            };
            let _ = repository_completion_tx.send(FrontendCompletion::Repository(
                RepositoryCompletion::BranchProgress {
                    workspace_id: workspace_id.clone(),
                    cancellation: cancellation.clone(),
                    message: progress_label,
                },
            ));

            let operation_handle = handle.clone();
            let operation = async move {
                match action {
                    BranchAction::Checkout(branch) => {
                        operation_handle.checkout_branch_async(&branch).await
                    }
                    BranchAction::Create(branch) => {
                        operation_handle.create_branch_async(&branch).await
                    }
                    BranchAction::Merge(branch) => {
                        operation_handle
                            .merge_branch_async(&branch)
                            .await
                            .and_then(|result| match result {
                                MergeResult::Success => {
                                    Ok("Merge completed.".to_string())
                                }
                                MergeResult::AlreadyUpToDate => {
                                    Ok("Already up to date.".to_string())
                                }
                                MergeResult::Conflicts(details) => Err(format!(
                                    "Merge stopped because of conflicts: {details}"
                                )),
                            })
                    }
                }
            };
            // Git mutations cannot be safely aborted once their blocking operation
            // starts. Keep the repository actor serialized until completion so a
            // History mutation cannot overlap a retired branch operation.
            let branch_result = operation.await;
            if cancellation.is_cancelled() {
                log::debug!(
                    "branch operation completed after workspace cancellation workspace={workspace_id}"
                );
                return;
            }
            let completion_message = match branch_result {
                Ok(message) => {
                    let message = if message.trim().is_empty() {
                        empty_success_message
                    } else {
                        message
                    };
                    log::info!(
                        "native branch operation completed workspace={} operation={} message={}",
                        workspace_id,
                        operation_name,
                        message.trim()
                    );
                    message
                }
                Err(message) => {
                    let _ = repository_completion_tx.send(
                        FrontendCompletion::Repository(
                            RepositoryCompletion::BranchFailed {
                                workspace_id: workspace_id.clone(),
                                cancellation: cancellation.clone(),
                                message,
                            },
                        ),
                    );
                    return;
                }
            };

            let reload_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                reload_handle.load_workspace_snapshot()
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "branch-final-refresh",
                task,
            )
            .await
            else {
                log::debug!(
                    "branch final refresh canceled workspace={workspace_id}"
                );
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::BranchFinished {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle,
                        result,
                        message: completion_message,
                    },
                ))
                .is_err()
            {
                log::warn!("branch completion dropped during shutdown");
            }
        }
        _ => unreachable!("repository request routed to the wrong handler"),
    }
}
