#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentActiveState {
    NewChat,
    Idle,
    Loading,
    Asking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentInactiveState {
    Dead,
    Unloaded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentSessionState {
    Active(AgentActiveState),
    Inactive(AgentInactiveState),
}
