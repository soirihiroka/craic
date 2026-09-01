async fn handle_repository_containers(
    request: RepositoryRequest,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    retired_jobs: RetiredJobSender,
) {
    match request {
        RepositoryRequest::LoadContainers {
            workspace_id,
            access,
            generation,
            cancellation,
        } => {
            let task = tokio::task::spawn_blocking(move || {
                docker::list_inventory(access.as_ref())
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "container-inventory",
                task,
            )
            .await
            else {
                log::debug!("container inventory canceled workspace={workspace_id}");
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Container inventory task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::Containers {
                        workspace_id,
                        generation,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("container inventory dropped during shutdown");
            }
        }
        RepositoryRequest::LoadContainerDetail {
            workspace_id,
            access,
            container_id,
            request_id,
            kind,
            cancellation,
        } => {
            let worker_id = container_id.clone();
            let task = tokio::task::spawn_blocking(move || {
                docker::inspect_container(access.as_ref(), &worker_id)
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "container-detail",
                task,
            )
            .await
            else {
                log::debug!(
                    "container detail canceled workspace={workspace_id} container={container_id}"
                );
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Container detail task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::ContainerDetail {
                        workspace_id,
                        container_id,
                        request_id,
                        kind,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("container detail dropped during shutdown");
            }
        }
        RepositoryRequest::RunContainerAction {
            workspace_id,
            workspace_generation,
            access,
            container_id,
            action,
            request_id,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "skipping canceled container action workspace={workspace_id} generation={} request={request_id}",
                    workspace_generation.get()
                );
                return;
            }
            let task = tokio::task::spawn_blocking(move || {
                docker::run_container_action(access.as_ref(), &container_id, action)
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "container-action",
                task,
            )
            .await
            else {
                log::debug!(
                    "container action canceled workspace={workspace_id} request={request_id}"
                );
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Container action task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::ContainerActionFinished {
                        workspace_id,
                        workspace_generation,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("container action completion dropped during shutdown");
            }
        }
        RepositoryRequest::RunComposeAction {
            workspace_id,
            workspace_generation,
            access,
            compose,
            action,
            request_id,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                log::debug!(
                    "skipping canceled Compose action workspace={workspace_id} generation={} request={request_id}",
                    workspace_generation.get()
                );
                return;
            }
            let task = tokio::task::spawn_blocking(move || {
                docker::run_compose_action(access.as_ref(), &compose, action)
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "compose-action",
                task,
            )
            .await
            else {
                log::debug!(
                    "compose action canceled workspace={workspace_id} request={request_id}"
                );
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Compose action task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::ContainerActionFinished {
                        workspace_id,
                        workspace_generation,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("compose action completion dropped during shutdown");
            }
        }
        _ => unreachable!("repository request routed to the wrong handler"),
    }
}
