use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OperationProgress {
    pub completed: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

pub struct OperationHandle<T> {
    progress: watch::Receiver<OperationProgress>,
    completion: oneshot::Receiver<Result<T, String>>,
    cancellation: CancellationToken,
}

impl<T> OperationHandle<T> {
    pub fn progress(&self) -> watch::Receiver<OperationProgress> {
        self.progress.clone()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub async fn complete(self) -> Result<T, String> {
        self.completion
            .await
            .unwrap_or_else(|_| Err("operation ended without a result".to_string()))
    }
}

pub struct OperationResultSender<T> {
    pub progress: watch::Sender<OperationProgress>,
    completion: Option<oneshot::Sender<Result<T, String>>>,
    pub cancellation: CancellationToken,
}

impl<T> OperationResultSender<T> {
    pub fn finish(mut self, result: Result<T, String>) -> Result<(), Result<T, String>> {
        self.completion
            .take()
            .expect("completion sender missing")
            .send(result)
    }
}

pub fn operation_channel<T>(
    parent: &CancellationToken,
) -> (OperationResultSender<T>, OperationHandle<T>) {
    let (progress_tx, progress_rx) = watch::channel(OperationProgress::default());
    let (completion_tx, completion_rx) = oneshot::channel();
    let cancellation = parent.child_token();
    (
        OperationResultSender {
            progress: progress_tx,
            completion: Some(completion_tx),
            cancellation: cancellation.clone(),
        },
        OperationHandle {
            progress: progress_rx,
            completion: completion_rx,
            cancellation,
        },
    )
}
