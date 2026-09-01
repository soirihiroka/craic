fn update_resource_usage(
    session: &AgentSession,
    process_running: bool,
    process_snapshot: Option<&ProcessSnapshot>,
    usage_tracker: &Rc<RefCell<ProcessUsageTracker>>,
    resource_usage_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64, Option<AgentResourceUsage>)>>>>,
) {
    let usage = if process_running {
        match (session.child_pid.get(), process_snapshot) {
            (Some(pid), Some(snapshot)) => {
                usage_tracker
                    .borrow_mut()
                    .sample(session.id, pid.0 as libc::pid_t, snapshot)
            }
            _ => {
                usage_tracker.borrow_mut().clear(session.id);
                None
            }
        }
    } else {
        usage_tracker.borrow_mut().clear(session.id);
        None
    };

    if let Some(ref cb) = *resource_usage_callback.borrow() {
        cb(session.id, usage);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SmartSummaryMode {
    Automatic,
    Manual,
}

fn maybe_start_smart_summary(
    session: &AgentSession,
    workspace_history: &Rc<RefCell<agent_history::WorkspaceKey>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    if session.state.get() != TerminalSessionState::Running {
        return;
    }

    let cursor_row = session.terminal.cursor_row().unwrap_or_default();
    if cursor_row < SMART_SUMMARY_TRIGGER_ROWS {
        return;
    }

    if start_smart_summary(
        session,
        workspace_history,
        history_callback,
        SmartSummaryMode::Automatic,
    )
    .is_err()
    {}
}

fn start_smart_summary(
    session: &AgentSession,
    workspace_history: &Rc<RefCell<agent_history::WorkspaceKey>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    mode: SmartSummaryMode,
) -> Result<(), String> {
    if session.summary_in_flight.get() {
        return Err("A smart summary is already running for this session.".to_string());
    }
    if mode == SmartSummaryMode::Automatic && session.summary_requested.get() {
        return Err("A smart summary was already requested for this session.".to_string());
    }
    if session.state.get() != TerminalSessionState::Running {
        return Err("The session terminal is not running.".to_string());
    }

    let local_id = ensure_agent_history_session(session, workspace_history, history_callback)
        .map_err(|err| {
            if mode == SmartSummaryMode::Automatic {
                session.summary_requested.set(true);
            }
            err
        })?;

    let existing_tags = match agent_history::lookup_session(local_id) {
        Ok(Some(row)) => {
            if mode == SmartSummaryMode::Automatic && row.task_description.is_some() {
                session.summary_requested.set(true);
                return Err("The session already has a smart summary.".to_string());
            }
            match agent_history::workspace_tags(&row.workspace_key) {
                Ok(tags) => tags,
                Err(err) => {
                    log::warn!(
                        "agent smart summary existing tags load failed local_id={} workspace_key={} error={}",
                        local_id,
                        row.workspace_key,
                        err
                    );
                    Vec::new()
                }
            }
        }
        Ok(None) => Vec::new(),
        Err(err) => {
            if mode == SmartSummaryMode::Automatic {
                return Err(format!("Smart summary history lookup failed: {err}"));
            }
            log::warn!("agent smart summary history lookup failed local_id={local_id}: {err}");
            Vec::new()
        }
    };

    let terminal_text = terminal_full_text(&session.terminal)
        .ok_or_else(|| "The session terminal has no transcript to summarize.".to_string())?;
    let cursor_row = session.terminal.cursor_row().unwrap_or_default();
    let title = session.label.text().to_string();
    let shell_provider_id = session.provider.provider_id().to_string();
    session.summary_in_flight.set(true);
    session.summary_requested.set(true);
    log::info!(
        "agent smart summary queued session_id={} local_id={} provider={} mode={:?} terminal_bytes={} cursor_row={}",
        session.id,
        local_id,
        shell_provider_id,
        mode,
        terminal_text.len(),
        cursor_row
    );

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result =
            smart_summary::generate(&shell_provider_id, &title, &terminal_text, &existing_tags)
                .and_then(|summary| {
                    agent_history::update_session_summary(local_id, &summary)?;
                    Ok(summary)
                });
        let _ = sender.send(result);
    });

    gtk::glib::timeout_add_local(std::time::Duration::from_millis(250), {
        let summary_in_flight = session.summary_in_flight.clone();
        let history_callback = history_callback.clone();

        move || match receiver.try_recv() {
            Ok(Ok(summary)) => {
                summary_in_flight.set(false);
                log::info!(
                    "agent smart summary complete local_id={} description_bytes={} tags={}",
                    local_id,
                    summary.task_description.len(),
                    summary.tags.len()
                );
                notify_history_changed(&history_callback);
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                summary_in_flight.set(false);
                log::warn!("agent smart summary failed local_id={local_id}: {err}");
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                summary_in_flight.set(false);
                log::warn!("agent smart summary worker disconnected local_id={local_id}");
                gtk::glib::ControlFlow::Break
            }
        }
    });

    Ok(())
}
