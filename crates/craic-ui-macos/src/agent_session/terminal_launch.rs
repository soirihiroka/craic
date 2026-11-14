fn prepare_terminal(
    workspace: &craic_config::ConfiguredWorkspace,
    provider_id: &str,
    history: Option<&agent_history::AgentSessionRow>,
) -> Result<TerminalCommand, String> {
    let access = craic_system::workspace::configured_workspace_access(workspace);
    let shell = access
        .provider
        .shell(&access.workspace)
        .ok_or_else(|| "Shell access is unavailable for this workspace".to_owned())?;
    let provider = craic_agent::terminal_provider::all_providers()
        .iter()
        .copied()
        .find(|provider| provider.provider_id() == provider_id)
        .ok_or_else(|| format!("Unknown agent provider: {provider_id}"))?;
    if let Some(history) = history {
        let id = history
            .cli_session_id
            .as_deref()
            .filter(|_| history.restore_state.is_restorable())
            .ok_or_else(|| "The original CLI session is unavailable".to_owned())?;
        provider.restore_command(Some(shell.as_ref()), &access.system, &access.workspace, id)
    } else {
        provider.command(Some(shell.as_ref()), &access.system, &access.workspace)
    }
}
