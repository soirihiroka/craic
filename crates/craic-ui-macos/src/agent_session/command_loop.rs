struct SummaryCancellation(craic_agent::agent_provider::CancellationToken);

impl Drop for SummaryCancellation {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub async fn run<F>(mut commands: mpsc::Receiver<Command>, emit: F)
where
    F: Fn(Event) + Send + Sync + 'static,
{
    let mut history_readers = Arc::new(Mutex::new(CliHistory::default()));
    let mut jobs = tokio::task::JoinSet::<Option<Event>>::new();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { jobs.abort_all(); return; };
                match command {
                    Command::CheckProviderUpdates { identity, generation, workspace, cancellation } => {
                        jobs.spawn(async move {
                            let task = tokio::task::spawn_blocking(move || {
                                let access = craic_system::workspace::configured_workspace_access(&workspace);
                                let shell = access.provider.shell(&access.workspace)
                                    .ok_or_else(|| "Shell access is unavailable for this workspace.".to_owned())?;
                                craic_agent::agent_updates::check_updates(shell, access.workspace)
                            });
                            let result = tokio::select! {
                                _ = cancellation.cancelled() => return None,
                                result = task => result.unwrap_or_else(|error| Err(format!("Provider update check failed: {error}"))),
                            };
                            Some(Event::ProviderUpdatesChecked { identity, generation, result })
                        });
                    }
                    Command::PrepareProviderUpdates { identity, generation, workspace, updates, cancellation } => {
                        jobs.spawn(async move {
                            let task = tokio::task::spawn_blocking(move || {
                                let access = craic_system::workspace::configured_workspace_access(&workspace);
                                let shell = access.provider.shell(&access.workspace)
                                    .ok_or_else(|| "Shell access is unavailable for this workspace.".to_owned())?;
                                craic_agent::agent_updates::update_command(shell.as_ref(), &access.workspace, &updates)
                            });
                            let result = tokio::select! {
                                _ = cancellation.cancelled() => return None,
                                result = task => result.unwrap_or_else(|error| Err(format!("Provider update launch failed: {error}"))),
                            };
                            Some(Event::ProviderUpdatePrepared { identity, generation, result })
                        });
                    }
                    Command::PrepareTerminal { identity, session_id, workspace, provider_id, history, cancellation } => {
                        jobs.spawn(async move {
                            let worker_provider = provider_id.clone();
                            let worker_history = history.clone();
                            let task = tokio::task::spawn_blocking(move || prepare_terminal(&workspace, &worker_provider, worker_history.as_ref()));
                            let result = tokio::select! {
                                _ = cancellation.cancelled() => return None,
                                result = tokio::time::timeout(std::time::Duration::from_secs(30), task) => match result {
                                    Ok(result) => result.unwrap_or_else(|error| Err(format!("Agent launch task failed: {error}"))),
                                    Err(_) => Err("Agent launch preparation timed out after 30 seconds.".to_owned()),
                                },
                            };
                            Some(Event::TerminalPrepared { identity, session_id, provider_id, history, result })
                        });
                    }
                    Command::GenerateSummary { identity, session_id, local_id, provider_id, title, transcript, automatic, cancellation } => {
                        jobs.spawn(async move {
                            let cancel_on_drop = SummaryCancellation(craic_agent::agent_provider::CancellationToken::new());
                            let generation_cancellation = cancel_on_drop.0.clone();
                            let task = tokio::task::spawn_blocking(move || {
                                let row = agent_history::lookup_session(local_id)?
                                    .ok_or_else(|| "Session history is unavailable".to_owned())?;
                                let tags = agent_history::workspace_tags(&row.workspace_key)?;
                                let summary = craic_agent::smart_summary::generate(&provider_id, &title, &transcript, &tags, &generation_cancellation)?;
                                if generation_cancellation.is_canceled() { return Err("Summary generation canceled".to_owned()); }
                                agent_history::update_session_summary(local_id, &summary)
                            });
                            let result = tokio::select! {
                                _ = cancellation.cancelled() => return None,
                                result = task => result.unwrap_or_else(|error| Err(format!("Summary task failed: {error}"))),
                            };
                            drop(cancel_on_drop);
                            Some(Event::SummaryFinished { identity, session_id, automatic, result })
                        });
                    }
                    Command::LoadHistory { identity, request_id, workspace, query, tags, limit, cancellation } => {
                        let workspace_id = identity.workspace_id.clone();
                        let readers = history_readers.clone();
                        jobs.spawn(async move {
                            let task = tokio::task::spawn_blocking(move || {
                                readers.lock().map_err(|_| "Agent history reader is unavailable".to_owned())?
                                    .load(&workspace_id, &workspace, &query, &tags, limit)
                            });
                            let result = tokio::select! {
                                _ = cancellation.cancelled() => return None,
                                result = tokio::time::timeout(std::time::Duration::from_secs(75), task) => match result {
                                    Ok(result) => result.unwrap_or_else(|error| Err(format!("Agent history task failed: {error}"))),
                                    Err(_) => Err("Agent history timed out after 75 seconds; the connection will be retried.".to_owned()),
                                },
                            };
                            Some(Event::HistoryLoaded { identity, request_id, result })
                        });
                    }
                    Command::Reset => {
                        jobs.abort_all();
                        history_readers = Arc::new(Mutex::new(CliHistory::default()));
                        log::debug!("native terminal-agent jobs reset for workspace transition");
                    }
                    Command::ResetHistory => {
                        history_readers = Arc::new(Mutex::new(CliHistory::default()));
                        log::info!("native agent history readers reset after an expired request");
                    }
                    Command::Shutdown { completed } => {
                        jobs.abort_all();
                        drop(history_readers);
                        if let Some(completed) = completed { let _ = completed.send(()); }
                        log::debug!("native terminal-agent service stopped");
                        return;
                    }
                }
            }
            completed = jobs.join_next(), if !jobs.is_empty() => {
                match completed {
                    Some(Ok(Some(event))) => {
                        if matches!(&event, Event::HistoryLoaded { result: Err(_), .. }) {
                            history_readers = Arc::new(Mutex::new(CliHistory::default()));
                        }
                        emit(event);
                    },
                    Some(Err(error)) if !error.is_cancelled() => log::warn!("native agent background job failed: {error}"),
                    _ => {}
                }
            }
        }
    }
}
