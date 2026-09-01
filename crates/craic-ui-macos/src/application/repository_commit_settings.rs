async fn handle_repository_commit_settings(
    request: RepositoryRequest,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    retired_jobs: RetiredJobSender,
) {
    match request {
        RepositoryRequest::SaveCommitAuthor {
            workspace_id,
            handle,
            option,
            cancellation,
        } => {
            let save_handle = handle.clone();
            let Some(saved) = wait_native_result(
                &cancellation,
                &retired_jobs,
                "save-commit-author",
                "Author update timed out.",
                async move {
                    save_handle
                        .save_author_identity_async(&option.name, &option.email)
                        .await
                },
            )
            .await
            else {
                return;
            };
            let result = match saved {
                Ok(()) => {
                    let reload_handle = handle.clone();
                    let task = tokio::task::spawn_blocking(move || {
                        reload_handle.load_workspace_snapshot()
                    });
                    let Some(result) = until_workspace_change(
                        &cancellation,
                        &retired_jobs,
                        "commit-author-refresh",
                        task,
                    )
                    .await
                    else {
                        log::debug!(
                            "commit author refresh canceled workspace={workspace_id}"
                        );
                        return;
                    };
                    result.unwrap_or_else(|error| {
                        Err(format!("Repository refresh task failed: {error}"))
                    })
                }
                Err(error) => Err(error),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::CommitAuthorFinished {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("commit author completion dropped during shutdown");
            }
        }
        RepositoryRequest::Commit {
            workspace_id,
            handle,
            summary,
            description,
            files,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "native commit discarded before start workspace={workspace_id}"
                );
                return;
            }
            log::info!(
                "native commit started workspace={} files={}",
                workspace_id,
                files.len()
            );
            let commit_handle = handle.clone();
            let result = commit_handle
                .commit_paths_async(&summary, &description, &files)
                .await;
            if cancellation.is_cancelled() {
                log::debug!(
                    "native commit completed after workspace cancellation workspace={workspace_id}"
                );
                return;
            }
            let succeeded = result.is_ok();
            log::info!(
                "native commit finished workspace={} success={}",
                workspace_id,
                succeeded
            );
            if !succeeded {
                if repository_completion_tx
                    .send(FrontendCompletion::Repository(
                            RepositoryCompletion::CommitFinished {
                                workspace_id,
                                cancellation: cancellation.clone(),
                                handle: None,
                            snapshot: None,
                            result,
                        },
                    ))
                    .is_err()
                {
                    log::warn!("commit failure dropped during shutdown");
                }
                return;
            }
            let reload_handle = handle.clone();
            let task = tokio::task::spawn_blocking(move || {
                reload_handle.load_workspace_snapshot()
            });
            let Some(snapshot) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "commit-refresh",
                task,
            )
            .await
            else {
                log::debug!("commit refresh canceled workspace={workspace_id}");
                return;
            };
            let snapshot = snapshot.unwrap_or_else(|error| {
                Err(format!("Repository refresh task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::CommitFinished {
                        workspace_id,
                        cancellation: cancellation.clone(),
                        handle: Some(handle),
                        snapshot: Some(snapshot),
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("commit completion dropped during shutdown");
            }
        }
        RepositoryRequest::GenerateCommitMessage {
            workspace_id,
            handle,
            files,
            request_id,
            cancellation,
            workspace_cancellation,
        } => {
            let task = tokio::task::spawn_blocking(craic_config::load);
            let Some(config) = until_workspace_change(
                &workspace_cancellation,
                &retired_jobs,
                "commit-message-config",
                task,
            )
            .await
            else {
                cancellation.cancel();
                log::debug!(
                    "commit-message config load canceled workspace={workspace_id}"
                );
                return;
            };
            let (provider_id, model) = match config {
                Ok(config) => {
                    (config.commit_message_provider, config.commit_message_model)
                }
                Err(error) => {
                    let result = Err(format!(
                        "Commit-message configuration could not be loaded: {error}"
                    ));
                    let _ = repository_completion_tx.send(
                        FrontendCompletion::Repository(
                            RepositoryCompletion::CommitMessageGenerated {
                                workspace_id,
                                cancellation: workspace_cancellation.clone(),
                                request_id,
                                provider_label: "configured provider".to_string(),
                                result,
                            },
                        ),
                    );
                    return;
                }
            };
            let provider_label = find_provider(&provider_id)
                .map(|provider| provider.label().to_string())
                .unwrap_or_else(|| provider_id.clone());
            let context_handle = handle.clone();
            let Some(context) = wait_native_result(
                &workspace_cancellation,
                &retired_jobs,
                "commit-message-context",
                "Commit-message context timed out.",
                async move {
                    context_handle.commit_message_context_async(&files).await
                },
            )
            .await
            else {
                log::debug!(
                    "commit-message context canceled workspace={workspace_id}"
                );
                return;
            };
            let result = match context {
                Ok(context) => {
                    let generation_cancellation = cancellation.clone();
                    let task = tokio::task::spawn_blocking(move || {
                        craic_agent::ai_commit::generate_from_context(
                            context,
                            &provider_id,
                            model.as_deref(),
                            &generation_cancellation,
                        )
                    });
                    let Some(result) = until_workspace_change(
                        &workspace_cancellation,
                        &retired_jobs,
                        "commit-message-generation",
                        task,
                    )
                    .await
                    else {
                        cancellation.cancel();
                        log::debug!(
                            "commit-message generation canceled workspace={workspace_id}"
                        );
                        return;
                    };
                    result.unwrap_or_else(|error| {
                        Err(format!(
                            "Commit-message generation task did not complete: {error}"
                        ))
                    })
                }
                Err(error) => Err(error),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::CommitMessageGenerated {
                        workspace_id,
                        cancellation: workspace_cancellation.clone(),
                        request_id,
                        provider_label,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("commit-message generation dropped during shutdown");
            }
        }
        RepositoryRequest::LoadCommitMessageSettings { request_id } => {
            let result = tokio::task::spawn_blocking(craic_config::load).await;
            match result {
                Ok(config) => {
                    let _ = repository_completion_tx.send(
                        FrontendCompletion::Repository(
                            RepositoryCompletion::CommitMessageSettingsLoaded {
                                request_id,
                                provider_id: config.commit_message_provider,
                                model: config.commit_message_model,
                            },
                        ),
                    );
                }
                Err(error) => {
                    let _ = repository_completion_tx.send(
                        FrontendCompletion::Repository(
                            RepositoryCompletion::CommitMessageSettingsFailed {
                                message: format!(
                                    "Commit-message settings could not be loaded: {error}"
                                ),
                            },
                        ),
                    );
                }
            }
        }
        RepositoryRequest::LoadCommitMessageModels {
            provider_id,
            selected_model,
            request_id,
        } => {
            let worker_provider = provider_id.clone();
            let result = tokio::task::spawn_blocking(move || {
                model_options(&worker_provider)
            })
            .await
            .unwrap_or_else(|error| {
                Err(format!("Provider model task did not complete: {error}"))
            });
            let _ = repository_completion_tx.send(FrontendCompletion::Repository(
                RepositoryCompletion::CommitMessageModelsLoaded {
                    request_id,
                    provider_id,
                    selected_model,
                    result,
                },
            ));
        }
        RepositoryRequest::SaveCommitMessageProvider { provider_id } => {
            if let Err(error) = tokio::task::spawn_blocking(move || {
                craic_config::save_commit_message_provider(&provider_id)
            })
            .await
            {
                let _ = repository_completion_tx.send(
                    FrontendCompletion::Repository(
                        RepositoryCompletion::CommitMessageSettingsFailed {
                            message: format!(
                                "Provider selection could not be saved: {error}"
                            ),
                        },
                    ),
                );
            }
        }
        RepositoryRequest::SaveCommitMessageModel { provider_id, model } => {
            if let Err(error) = tokio::task::spawn_blocking(move || {
                craic_config::save_commit_message_model(
                    &provider_id,
                    model.as_deref(),
                )
            })
            .await
            {
                let _ = repository_completion_tx.send(
                    FrontendCompletion::Repository(
                        RepositoryCompletion::CommitMessageSettingsFailed {
                            message: format!(
                                "Model selection could not be saved: {error}"
                            ),
                        },
                    ),
                );
            }
        }
        RepositoryRequest::LoadWorkspaceSettings {
            workspace_id,
            request_id,
            workspace,
            handle,
            cancellation,
        } => {
            let Some(result) = wait_native_result(
                &cancellation,
                &retired_jobs,
                "workspace-settings-load",
                "Workspace settings loading timed out.",
                async move {
                    let (settings, github_accounts) = tokio::join!(
                        handle.settings_async(),
                        load_native_workspace_github_accounts(workspace)
                    );
                    settings.map(|settings| NativeWorkspaceSettings {
                        settings,
                        github_accounts,
                    })
                },
            )
            .await
            else {
                return;
            };
            let _ = repository_completion_tx.send(FrontendCompletion::Repository(
                RepositoryCompletion::WorkspaceSettingsLoaded {
                    workspace_id,
                    cancellation: cancellation.clone(),
                    request_id,
                    result,
                },
            ));
        }
        RepositoryRequest::SaveWorkspaceSettings {
            workspace_id,
            request_id,
            handle,
            settings,
            cancellation,
        } => {
            let save_handle = handle.clone();
            let Some(save_result) = wait_native_result(
                &cancellation,
                &retired_jobs,
                "workspace-settings-save",
                "Workspace settings save timed out.",
                async move { save_handle.save_settings_async(&settings).await },
            )
            .await
            else {
                return;
            };
            let result = match save_result {
                Ok(()) => {
                    let reload_handle = handle.clone();
                    let task = tokio::task::spawn_blocking(move || {
                        reload_handle.load_workspace_snapshot()
                    });
                    let Some(result) = until_workspace_change(
                        &cancellation,
                        &retired_jobs,
                        "workspace-settings-refresh",
                        task,
                    )
                    .await
                    else {
                        log::debug!(
                            "workspace settings refresh canceled workspace={workspace_id}"
                        );
                        return;
                    };
                    result.unwrap_or_else(|error| {
                        Err(format!(
                            "Workspace reload after settings save did not complete: {error}"
                        ))
                    })
                }
                Err(error) => Err(error),
            };
            let _ = repository_completion_tx.send(FrontendCompletion::Repository(
                RepositoryCompletion::WorkspaceSettingsSaved {
                    workspace_id,
                    cancellation: cancellation.clone(),
                    request_id,
                    handle,
                    result,
                },
            ));
        }
        _ => unreachable!("repository request routed to the wrong handler"),
    }
}
