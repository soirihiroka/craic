use crate::AppChannels;
use crate::UiEvent;
use crate::actor;
use craic_platform::{UiContextId, UiEffect, UiEffectId, UiEffectRequest, UiEffectResult};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub worker_threads: usize,
    pub thread_name: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            worker_threads: 2,
            thread_name: "craic-app".to_string(),
        }
    }
}

pub struct ApplicationRuntime {
    runtime: Option<Runtime>,
    handle: crate::AppHandle,
    tracker: TaskTracker,
    root_cancellation: CancellationToken,
    workspace_cancellation: Arc<Mutex<CancellationToken>>,
    shutdown_request: CancellationToken,
    shutdown_ready: std::sync::mpsc::Receiver<()>,
    events: mpsc::Sender<UiEvent>,
    effect_waiters: actor::EffectWaiters,
}

#[derive(Clone)]
pub struct UiEffectClient {
    events: mpsc::Sender<UiEvent>,
    effect_waiters: actor::EffectWaiters,
}

impl UiEffectClient {
    pub async fn request(&self, context: UiContextId, effect: UiEffect) -> UiEffectResult {
        let id = UiEffectId::new();
        let (result_tx, result_rx) = oneshot::channel();
        self.effect_waiters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), result_tx);
        if self
            .events
            .send(UiEvent::Effect(UiEffectRequest {
                id: id.clone(),
                context,
                effect,
            }))
            .await
            .is_err()
        {
            self.effect_waiters
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
            return UiEffectResult::Failed("The native UI event channel is closed".to_string());
        }
        result_rx.await.unwrap_or(UiEffectResult::Cancelled)
    }
}

impl ApplicationRuntime {
    pub fn start(config: RuntimeConfig) -> Result<(Self, AppChannels), std::io::Error> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(config.worker_threads.max(1))
            .thread_name(config.thread_name)
            .enable_all()
            .build()?;
        let root_cancellation = CancellationToken::new();
        let workspace_cancellation = Arc::new(Mutex::new(root_cancellation.child_token()));
        let tracker = TaskTracker::new();
        let (channels, command_rx, event_tx, shutdown_request) =
            actor::channels(root_cancellation.clone(), workspace_cancellation.clone());
        let effect_waiters = Arc::new(Mutex::new(HashMap::new()));
        let (shutdown_ready_tx, shutdown_ready) = std::sync::mpsc::sync_channel(1);
        runtime.spawn(actor::run(
            command_rx,
            event_tx.clone(),
            root_cancellation.clone(),
            tracker.clone(),
            shutdown_request.clone(),
            shutdown_ready_tx,
            effect_waiters.clone(),
        ));
        let owner = Self {
            runtime: Some(runtime),
            handle: channels.handle.clone(),
            tracker,
            root_cancellation,
            workspace_cancellation,
            shutdown_request,
            shutdown_ready,
            events: event_tx,
            effect_waiters,
        };
        Ok((owner, channels))
    }

    pub fn handle(&self) -> crate::AppHandle {
        self.handle.clone()
    }

    pub fn ui_effect_client(&self) -> UiEffectClient {
        UiEffectClient {
            events: self.events.clone(),
            effect_waiters: self.effect_waiters.clone(),
        }
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.root_cancellation.clone()
    }

    pub fn child_token(&self) -> CancellationToken {
        self.workspace_cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .child_token()
    }

    pub async fn request_ui_effect(
        &self,
        context: UiContextId,
        effect: UiEffect,
    ) -> UiEffectResult {
        self.ui_effect_client().request(context, effect).await
    }

    pub fn spawn<F>(&self, future: F) -> Result<(), &'static str>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let Some(runtime) = self.runtime.as_ref() else {
            return Err("application runtime has shut down");
        };
        self.tracker.spawn_on(future, runtime.handle());
        Ok(())
    }

    pub fn shutdown(mut self, timeout: Duration) {
        let started = Instant::now();
        self.shutdown_request.cancel();
        log::info!(
            "application runtime shutdown started timeout_ms={}",
            timeout.as_millis()
        );
        if self.shutdown_ready.recv_timeout(timeout).is_err() {
            log::warn!(
                "application runtime shutdown handshake timed out timeout_ms={}",
                timeout.as_millis()
            );
        }
        self.root_cancellation.cancel();
        self.tracker.close();
        let remaining = timeout.saturating_sub(started.elapsed());
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(remaining);
        }
        log::info!("application runtime shutdown complete");
    }
}

impl Drop for ApplicationRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            self.root_cancellation.cancel();
            self.tracker.close();
            log::warn!("application runtime dropped without explicit shutdown");
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}
