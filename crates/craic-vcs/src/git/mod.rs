mod access;
mod diff;
mod remote;
mod types;

pub use crate::CommitMessageContext;
pub use access::{
    BackgroundPullSubscription, ChangeListener, ChangeListenerSubscription, FileDiffSubscription,
    GitOperationHook, GitOperationPostHook, GitRepoHandle, OperationCallback,
    clone_repository_with_shell,
};
pub use diff::*;
pub use remote::*;
pub use types::*;

pub const MAX_TEXT_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
