#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentActiveState {
    NewChat,
    Idle,
    Loading,
    Asking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentInactiveState {
    Dead,
    Unloaded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentSessionState {
    Active(AgentActiveState),
    Inactive(AgentInactiveState),
}
