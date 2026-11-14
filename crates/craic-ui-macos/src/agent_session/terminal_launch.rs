fn prepare_terminal(
    workspace: &craic_config::ConfiguredWorkspace,
    provider_id: &str,
    history: Option<&agent_history::AgentSessionRow>,
) -> Result<craic_system::system::capabilities::shell::ShellCommandSpec, String> {
    use craic_system::system::capabilities::shell::ShellCommandSpec;
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
    let command = if let Some(history) = history {
        let id = history
            .cli_session_id
            .as_deref()
            .filter(|_| history.restore_state.is_restorable())
            .ok_or_else(|| "The original CLI session is unavailable".to_owned())?;
        provider.restore_command(Some(shell.as_ref()), &access.system, &access.workspace, id)?
    } else {
        provider.command(Some(shell.as_ref()), &access.system, &access.workspace)?
    };
    let mut argv = command.argv().into_iter();
    let program = argv
        .next()
        .ok_or_else(|| "Agent command is empty".to_owned())?;
    let args = argv
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if access.provider.kind() == ProviderKind::Local {
        // The login shell supplies env-based interpreters for desktop launches on macOS.
        return shell.command(&access.workspace.root, &program.to_string_lossy(), &args);
    }
    let mut working_dir = access.workspace.root;
    working_dir.absolute = command.working_dir().to_owned();
    let mut spec = ShellCommandSpec::new(program, working_dir);
    spec.args = args.into_iter().map(Into::into).collect();
    Ok(spec)
}
