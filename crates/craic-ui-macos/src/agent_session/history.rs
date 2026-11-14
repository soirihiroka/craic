#[derive(Default)]
struct CliHistory {
    workspace_id: Option<String>,
    codex: Option<agent_history::CodexHistoryReader>,
    grok: Option<agent_history::GrokHistoryReader>,
}

impl CliHistory {
    fn load(
        &mut self,
        workspace_id: &str,
        workspace: &craic_config::ConfiguredWorkspace,
        query: &str,
        tags: &[String],
        limit: usize,
    ) -> Result<CliHistoryResult, String> {
        if self.workspace_id.as_deref() != Some(workspace_id) {
            *self = Self {
                workspace_id: Some(workspace_id.to_owned()),
                ..Self::default()
            };
        }
        let access = craic_system::workspace::configured_workspace_access(workspace);
        // GTK only imports native CLI history for local workspaces. In particular,
        // never mix the local OpenCode database into an SSH workspace's history.
        if access.provider.kind() != ProviderKind::Local {
            return Ok(CliHistoryResult {
                rows: Vec::new(),
                tags: Vec::new(),
                warnings: Vec::new(),
            });
        }
        let shell = access
            .provider
            .shell(&access.workspace)
            .ok_or_else(|| "Shell access is unavailable for this workspace".to_owned())?;
        let codex = (|| {
            if self.codex.is_none() {
                let config = craic_agent::app_server::config(
                    shell.as_ref(),
                    &access.workspace,
                    access.provider.kind(),
                )?;
                self.codex = Some(agent_history::CodexHistoryReader::new(config)?);
            }
            Ok(self.codex.as_ref().unwrap().clone())
        })();
        let grok = (|| {
            if self.grok.is_none() {
                let program = shell
                    .which("grok")?
                    .ok_or_else(|| "Grok CLI is not installed".to_owned())?;
                let command = shell.fast_command(
                    &access.workspace.root,
                    &program,
                    &["agent".to_owned(), "stdio".to_owned()],
                )?;
                self.grok = Some(agent_history::GrokHistoryReader::new(command)?);
            }
            Ok(self.grok.as_ref().unwrap().clone())
        })();
        let key =
            agent_history::workspace_for_system_path(workspace_id, &access.workspace.root.absolute);
        let history = agent_history::list_native_sessions(
            &key,
            limit,
            Some(query),
            tags,
            codex.as_ref().map_err(Clone::clone),
            grok.as_ref().map_err(Clone::clone),
        )?;
        Ok(CliHistoryResult {
            rows: history.rows,
            tags: history.tags,
            warnings: history.warnings,
        })
    }
}
