mod access;
mod diff;
mod remote;
mod types;

pub(crate) use access::{
    BackgroundPullSubscription, ChangeListener, ChangeListenerSubscription, GitOperationHook,
    GitOperationPostHook, GitRepoHandle, OperationCallback, clone_repository_with_shell,
};
pub(crate) use diff::*;
pub(crate) use remote::*;
pub use types::*;

pub(crate) const MAX_TEXT_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
