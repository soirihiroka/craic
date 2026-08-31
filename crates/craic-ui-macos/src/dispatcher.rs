use craic_platform::{MainThreadDispatcher, UiDispatchError};
use dispatch2::DispatchQueue;

#[derive(Clone, Copy, Debug, Default)]
pub struct AppKitDispatcher;

impl MainThreadDispatcher for AppKitDispatcher {
    fn schedule(&self, job: Box<dyn FnOnce() + Send>) -> Result<(), UiDispatchError> {
        DispatchQueue::main().exec_async(job);
        Ok(())
    }
}
