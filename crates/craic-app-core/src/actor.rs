use crate::{AppCommand, ApplicationViewState, UiEvent};
use craic_platform::{UiEffectId, UiEffectResult};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub const APP_COMMAND_CAPACITY: usize = 128;
pub const UI_EVENT_CAPACITY: usize = 128;
pub(crate) type EffectWaiters = Arc<Mutex<HashMap<UiEffectId, oneshot::Sender<UiEffectResult>>>>;

pub struct AppChannels {
    pub handle: AppHandle,
    pub events: mpsc::Receiver<UiEvent>,
}

#[derive(Clone)]
pub struct AppHandle {
    commands: mpsc::Sender<AppCommand>,
    shutdown_request: CancellationToken,
    root_cancellation: CancellationToken,
    workspace_cancellation: Arc<Mutex<CancellationToken>>,
}

impl AppHandle {
    pub async fn send(&self, command: AppCommand) -> Result<(), AppCommand> {
        let rotates_workspace = matches!(command, AppCommand::SelectWorkspace(_));
        self.commands.send(command).await.map_err(|error| error.0)?;
        if rotates_workspace {
            self.rotate_workspace_cancellation();
        }
        Ok(())
    }

    pub fn try_send(&self, command: AppCommand) -> Result<(), AppCommand> {
        let rotates_workspace = matches!(command, AppCommand::SelectWorkspace(_));
        self.commands
            .try_send(command)
            .map_err(|error| error.into_inner())?;
        if rotates_workspace {
            self.rotate_workspace_cancellation();
        }
        Ok(())
    }

    pub fn request_shutdown(&self) {
        self.shutdown_request.cancel();
    }

    pub fn workspace_cancellation_token(&self) -> CancellationToken {
        self.workspace_cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .child_token()
    }

    fn rotate_workspace_cancellation(&self) {
        let mut workspace = self
            .workspace_cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        workspace.cancel();
        *workspace = self.root_cancellation.child_token();
    }
}

pub(crate) fn channels(
    root_cancellation: CancellationToken,
    workspace_cancellation: Arc<Mutex<CancellationToken>>,
) -> (
    AppChannels,
    mpsc::Receiver<AppCommand>,
    mpsc::Sender<UiEvent>,
    CancellationToken,
) {
    let (command_tx, command_rx) = mpsc::channel(APP_COMMAND_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel(UI_EVENT_CAPACITY);
    let shutdown_request = CancellationToken::new();
    (
        AppChannels {
            handle: AppHandle {
                commands: command_tx,
                shutdown_request: shutdown_request.clone(),
                root_cancellation,
                workspace_cancellation,
            },
            events: event_rx,
        },
        command_rx,
        event_tx,
        shutdown_request,
    )
}

pub(crate) async fn run(
    mut commands: mpsc::Receiver<AppCommand>,
    events: mpsc::Sender<UiEvent>,
    root_cancellation: CancellationToken,
    tracker: TaskTracker,
    shutdown_request: CancellationToken,
    shutdown_ready: std::sync::mpsc::SyncSender<()>,
    effect_waiters: EffectWaiters,
) {
    let mut state = ApplicationViewState::default();
    if events
        .send(UiEvent::ApplicationState(Arc::new(state.clone())))
        .await
        .is_err()
    {
        root_cancellation.cancel();
        tracker.close();
        tracker.wait().await;
        let _ = shutdown_ready.try_send(());
        return;
    }

    loop {
        let command = tokio::select! {
            biased;
            _ = shutdown_request.cancelled() => AppCommand::ShutdownRequested,
            command = commands.recv() => match command {
                Some(command) => command,
                None => break,
            },
        };
        let changed = match command {
            AppCommand::ActivatePage(page) => {
                state.active_page = Some(page);
                true
            }
            AppCommand::SelectWorkspace(selection) => {
                state.workspace_generation = state.workspace_generation.next();
                log::info!(
                    "application workspace generation activated workspace={} generation={}",
                    selection.id.as_str(),
                    state.workspace_generation.get()
                );
                state.workspace = Some(selection);
                state.refreshing.clear();
                true
            }
            AppCommand::Refresh(scope) => {
                if !state.refreshing.contains(&scope) {
                    state.refreshing.push(scope);
                    true
                } else {
                    false
                }
            }
            AppCommand::ServiceCompleted(completion) => {
                let is_current = state
                    .workspace
                    .as_ref()
                    .is_none_or(|_| state.workspace_generation == completion.generation());
                if !is_current {
                    log::debug!(
                        "ignored stale service completion generation={}",
                        completion.generation().get()
                    );
                }
                is_current
            }
            AppCommand::CompleteUiEffect(completion) => {
                let waiter = effect_waiters
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&completion.id);
                if let Some(waiter) = waiter {
                    let _ = waiter.send(completion.result);
                } else {
                    log::debug!("ignored stale or unknown UI effect completion");
                }
                false
            }
            AppCommand::RoutePageCommand(_) => false,
            AppCommand::ShutdownRequested => {
                state.shutting_down = true;
                root_cancellation.cancel();
                tracker.close();
                for (_, waiter) in effect_waiters
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .drain()
                {
                    let _ = waiter.send(UiEffectResult::Cancelled);
                }
                let _ = events
                    .send(UiEvent::ApplicationState(Arc::new(state)))
                    .await;
                tracker.wait().await;
                let _ = events.send(UiEvent::ShutdownReady).await;
                let _ = shutdown_ready.try_send(());
                log::info!("application actor shutdown ready");
                return;
            }
        };

        if changed
            && events
                .send(UiEvent::ApplicationState(Arc::new(state.clone())))
                .await
                .is_err()
        {
            root_cancellation.cancel();
            tracker.close();
            tracker.wait().await;
            let _ = shutdown_ready.try_send(());
            return;
        }
    }

    root_cancellation.cancel();
    tracker.close();
    tracker.wait().await;
    let _ = shutdown_ready.try_send(());
    log::info!("application actor command channel closed");
}
