#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdentity {
    pub workspace_id: String,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub smart_summary: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub updated_at: i64,
    pub pinned: bool,
    pub archived: bool,
}

#[derive(Debug)]
pub struct CliHistoryResult {
    pub rows: Vec<agent_history::AgentSessionRow>,
    pub tags: Vec<agent_history::WorkspaceTag>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum Command {
    CheckProviderUpdates {
        identity: SessionIdentity,
        generation: u64,
        workspace: craic_config::ConfiguredWorkspace,
        cancellation: CancellationToken,
    },
    PrepareProviderUpdates {
        identity: SessionIdentity,
        generation: u64,
        workspace: craic_config::ConfiguredWorkspace,
        updates: Vec<craic_agent::agent_updates::ProviderUpdate>,
        cancellation: CancellationToken,
    },
    PrepareTerminal {
        identity: SessionIdentity,
        session_id: isize,
        workspace: craic_config::ConfiguredWorkspace,
        provider_id: String,
        history: Option<agent_history::AgentSessionRow>,
        cancellation: CancellationToken,
    },
    GenerateSummary {
        identity: SessionIdentity,
        session_id: isize,
        local_id: i64,
        provider_id: String,
        title: String,
        transcript: String,
        automatic: bool,
        cancellation: CancellationToken,
    },
    LoadHistory {
        identity: SessionIdentity,
        request_id: u64,
        workspace: craic_config::ConfiguredWorkspace,
        query: String,
        tags: Vec<String>,
        limit: usize,
        cancellation: CancellationToken,
    },
    Reset,
    ResetHistory,
    Shutdown {
        completed: Option<std::sync::mpsc::SyncSender<()>>,
    },
}

#[derive(Debug)]
pub enum Event {
    ProviderUpdatesChecked {
        identity: SessionIdentity,
        generation: u64,
        result: Result<craic_agent::agent_updates::ProviderUpdateCheck, String>,
    },
    ProviderUpdatePrepared {
        identity: SessionIdentity,
        generation: u64,
        result: Result<ShellCommandSpec, String>,
    },
    TerminalPrepared {
        identity: SessionIdentity,
        session_id: isize,
        provider_id: String,
        history: Option<agent_history::AgentSessionRow>,
        result: Result<TerminalCommand, String>,
    },
    SummaryFinished {
        identity: SessionIdentity,
        session_id: isize,
        automatic: bool,
        result: Result<(), String>,
    },
    HistoryLoaded {
        identity: SessionIdentity,
        request_id: u64,
        result: Result<CliHistoryResult, String>,
    },
}
