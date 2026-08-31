use crate::{
    ActionId, AppCommand, ApplicationViewState, Badge, Generation, PAGE_DESCRIPTORS, PageCommand,
    PageId, PageRefreshCoordinator, PageServiceRequest, PageViewState, RefreshScope,
    ServiceCompletion, UiEvent,
};
use craic_platform::{UiEffectId, UiEffectResult};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub const APP_COMMAND_CAPACITY: usize = 128;
pub const UI_EVENT_CAPACITY: usize = 128;
pub(crate) type EffectWaiters = Arc<Mutex<HashMap<UiEffectId, oneshot::Sender<UiEffectResult>>>>;

#[derive(Clone)]
struct PendingPageRequest {
    page: PageId,
    workspace_generation: Generation,
    page_generation: Generation,
}

struct PageStateOwner {
    refreshes: PageRefreshCoordinator,
    states: BTreeMap<PageId, PageViewState>,
    revisions: BTreeMap<PageId, u64>,
    pending: HashMap<uuid::Uuid, PendingPageRequest>,
}

impl PageStateOwner {
    fn new() -> Self {
        let pages = PAGE_DESCRIPTORS.map(|descriptor| descriptor.page_id());
        Self {
            refreshes: PageRefreshCoordinator::new(pages.clone()),
            states: PAGE_DESCRIPTORS
                .into_iter()
                .map(|descriptor| {
                    (
                        descriptor.page_id(),
                        PageViewState {
                            title: descriptor.label.to_string(),
                            ..PageViewState::default()
                        },
                    )
                })
                .collect(),
            revisions: pages.into_iter().map(|page| (page, 0)).collect(),
            pending: HashMap::new(),
        }
    }

    fn begin(
        &mut self,
        mut command: PageCommand,
        page: PageId,
        workspace_generation: Generation,
    ) -> PageServiceRequest {
        command.page = Some(page.clone());
        self.pending.retain(|_, request| request.page != page);
        let page_generation = self.refreshes.begin(&page);
        self.state_mut(&page).refreshing = true;
        let request_id = uuid::Uuid::new_v4();
        self.pending.insert(
            request_id,
            PendingPageRequest {
                page,
                workspace_generation,
                page_generation,
            },
        );
        PageServiceRequest {
            request_id,
            workspace_generation,
            page_generation,
            command,
        }
    }

    fn is_refreshing(&self, page: &PageId) -> bool {
        self.refreshes.is_refreshing(page)
    }

    fn complete(
        &mut self,
        completion: ServiceCompletion,
        workspace_generation: Generation,
    ) -> Option<PageId> {
        let request_id = completion.request_id();
        let request = self.pending.get(&request_id)?;
        if request.workspace_generation != workspace_generation
            || request.page_generation != completion.generation()
            || !self
                .refreshes
                .is_current(&request.page, request.page_generation)
        {
            log::debug!(
                "ignored stale page service completion request={} page={} generation={}",
                request_id,
                request.page.as_str(),
                completion.generation().get()
            );
            return None;
        }

        let request = self
            .pending
            .remove(&request_id)
            .expect("validated page request remains pending");
        let page = request.page;
        let finished = self.refreshes.finish(&page, request.page_generation);
        debug_assert!(finished, "validated page refresh must finish");
        let state = self.state_mut(&page);
        state.refreshing = false;
        match completion {
            ServiceCompletion::Succeeded { payload, .. } => state.data = payload,
            ServiceCompletion::Failed { message, .. } => {
                state.data = json!({ "error": message });
            }
            ServiceCompletion::Cancelled { .. } => {}
        }
        Some(page)
    }

    fn set_badge(&mut self, page: &PageId, badge: Option<Badge>) {
        self.state_mut(page).badge = badge;
    }

    fn invalidate_workspace(&mut self) -> Vec<PageId> {
        self.pending.clear();
        self.refreshes.cancel_all();
        let pages = self.states.keys().cloned().collect::<Vec<_>>();
        for page in &pages {
            let state = self.state_mut(page);
            state.refreshing = false;
            state.badge = None;
            state.data = Value::Null;
        }
        pages
    }

    fn state_mut(&mut self, page: &PageId) -> &mut PageViewState {
        self.states
            .entry(page.clone())
            .or_insert_with(|| PageViewState {
                title: page.as_str().to_string(),
                ..PageViewState::default()
            })
    }

    fn event(&mut self, page: &PageId) -> UiEvent {
        let state = Arc::new(self.state_mut(page).clone());
        let revision = self.revisions.entry(page.clone()).or_default();
        *revision = revision.saturating_add(1);
        UiEvent::PageState {
            page: page.clone(),
            revision: *revision,
            state,
        }
    }
}

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
    let mut pages = PageStateOwner::new();
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
        let mut page_events = Vec::new();
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
                state.badges.clear();
                for page in pages.invalidate_workspace() {
                    page_events.push(pages.event(&page));
                }
                true
            }
            AppCommand::Refresh(scope) => {
                let targets = match &scope {
                    RefreshScope::Page(page) => vec![page.clone()],
                    RefreshScope::Application | RefreshScope::Workspace => PAGE_DESCRIPTORS
                        .into_iter()
                        .map(|descriptor| descriptor.page_id())
                        .collect(),
                };
                let mut requested = false;
                for page in targets {
                    if pages.is_refreshing(&page) {
                        log::debug!("coalesced page refresh page={}", page.as_str());
                        continue;
                    }
                    let request = pages.begin(
                        PageCommand {
                            page: Some(page.clone()),
                            action: ActionId::new("refresh"),
                            payload: Value::Null,
                        },
                        page.clone(),
                        state.workspace_generation,
                    );
                    set_page_refreshing(&mut state, &page, true);
                    page_events.push(pages.event(&page));
                    page_events.push(UiEvent::PageServiceRequest(request));
                    requested = true;
                }
                requested
            }
            AppCommand::ServiceCompleted(completion) => {
                if let Some(page) = pages.complete(completion, state.workspace_generation) {
                    set_page_refreshing(&mut state, &page, false);
                    sync_page_badge(&mut state, &pages, &page);
                    page_events.push(pages.event(&page));
                    true
                } else {
                    false
                }
            }
            AppCommand::SetPageBadge { page, badge } => {
                pages.set_badge(&page, badge);
                sync_page_badge(&mut state, &pages, &page);
                page_events.push(pages.event(&page));
                true
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
            AppCommand::RoutePageCommand(command) => {
                let target = command.page.clone().or_else(|| state.active_page.clone());
                if let Some(page) = target {
                    if pages.is_refreshing(&page) {
                        log::debug!(
                            "coalesced routed page command page={} action={}",
                            page.as_str(),
                            command.action.as_str()
                        );
                        false
                    } else {
                        let request =
                            pages.begin(command, page.clone(), state.workspace_generation);
                        set_page_refreshing(&mut state, &page, true);
                        page_events.push(pages.event(&page));
                        page_events.push(UiEvent::PageServiceRequest(request));
                        true
                    }
                } else {
                    log::warn!(
                        "page command ignored because it has no explicit target and no page is active"
                    );
                    false
                }
            }
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

        if changed {
            page_events.push(UiEvent::ApplicationState(Arc::new(state.clone())));
        }
        let mut event_channel_closed = false;
        for event in page_events {
            if events.send(event).await.is_err() {
                event_channel_closed = true;
                break;
            }
        }
        if event_channel_closed {
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

fn set_page_refreshing(state: &mut ApplicationViewState, page: &PageId, refreshing: bool) {
    let scope = RefreshScope::Page(page.clone());
    state.refreshing.retain(|candidate| candidate != &scope);
    if refreshing {
        state.refreshing.push(scope);
    }
}

fn sync_page_badge(state: &mut ApplicationViewState, pages: &PageStateOwner, page: &PageId) {
    let badge = pages
        .states
        .get(page)
        .and_then(|page_state| page_state.badge.clone());
    if let Some(badge) = badge {
        state.badges.insert(page.clone(), badge);
    } else {
        state.badges.remove(page);
    }
}
