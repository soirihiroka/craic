mod access;
mod diagnostics;
mod diff;
mod remote;
mod suggestions;
mod types;

pub use crate::CommitMessageContext;
pub use access::{
    BackgroundPullSubscription, ChangeListener, ChangeListenerSubscription, FileDiffReceiver,
    FileDiffSubscription, GitCommandEvent, GitCommandGenerator, GitOperationHook,
    GitOperationPostHook, GitOperationReceiver, GitRepoHandle, clone_repository_with_shell,
};
pub use diagnostics::{
    is_local_changes_overwritten_error, local_changes_overwritten_body,
    parse_files_to_be_overwritten,
};
pub use diff::*;
pub use remote::*;
pub use suggestions::*;
pub use types::*;

pub const MAX_TEXT_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
